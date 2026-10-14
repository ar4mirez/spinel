# The program's own state: its name and arguments, and what runs at exit
# (#145, which needs them to run mspec). `Process` itself is #43.

# How Spinel identifies itself: the Ruby it implements, and itself as the
# engine. The same constants `spinel --version` prints.
__ruby_constants__.each_slice(2) { |name, value| Object.const_set(name, value.freeze) }
RUBY_PATCHLEVEL = 0
RUBY_RELEASE_DATE = "2025-12-25".freeze
RUBY_REVISION = "spinel".freeze
RUBY_COPYRIGHT = "ruby - Copyright (C) 1993-2025 Yukihiro Matsumoto".freeze

ARGV = __argv__
$0 = ARGV.shift
alias $PROGRAM_NAME $0
# The process's id, kept current by `fork`; read-only to a program.
__global_store__(:$$, __sys_ids__[0])
__freeze_global__(:$$)

module Kernel
  def at_exit(&block)
    raise ArgumentError, "called without a block" unless block
    Kernel.__at_exit__.push(block)
    block
  end

  module_function :at_exit

  def self.__at_exit__
    @__at_exit__ ||= []
  end

  # Run the `at_exit` blocks, last registered first, as the program ends.
  # A block that raises `SystemExit` changes the status; any other exception
  # is reported and the rest still run. Answers the exit status.
  def self.__exit__(error)
    status = error.nil? ? 0 : __status_of__(error)
    handlers = __at_exit__
    until handlers.empty?
      handler = handlers.pop
      begin
        handler.call
      rescue SystemExit => e
        status = e.status
      rescue Exception => e
        status = 1
        $stderr.write(e.full_message)
      end
    end
    status
  end

  def self.__status_of__(error)
    SystemExit === error ? error.status : 1
  end
end


# The environment, as `ENV`. Read from the process on first use and kept per
# heap: a write is seen by this program and not by the process, which is the
# same thing until `Process` (#43) can start a child that would inherit it.
ENV = Object.new

class << ENV
  include Enumerable

  # The whole environment as a Hash, built the first time something
  # enumerates it. Until then one variable is read straight from the process
  # and a write is kept in `@written` — building the table costs milliseconds,
  # and a spec runner touches `ENV` in every example's heap.
  def __table__
    @table ||= begin
      table = {}
      pairs = __environ__
      i = 0
      while i < pairs.size
        table[pairs[i].freeze] = pairs[i + 1].freeze
        i += 2
      end
      (@written || {}).each { |name, value| value.nil? ? table.delete(name) : table[name] = value }
      @written = nil
      table
    end
  end

  def __lookup__(name)
    return @table[name] if @table
    return @written[name] if @written&.key?(name)
    __getenv__(name)&.freeze
  end

  def __store__(name, value)
    if @table
      value.nil? ? @table.delete(name) : @table[name] = value
    else
      (@written ||= {})[name] = value
    end
  end

  def __name__(name)
    return name if String === name
    raise TypeError, "no implicit conversion of #{name.nil? ? "nil" : name.class} into String" unless name.respond_to?(:to_str)
    name.to_str
  end

  def [](name)
    __external__(__lookup__(__name__(name)))
  end

  # A value as a program reads it: transcoded to `Encoding.default_internal`
  # when one is set.
  def __external__(value)
    internal = Encoding.default_internal
    value.nil? || internal.nil? ? value : value.encode(internal).freeze
  end

  # `setenv(3)`'s rule for a name, measured: empty, or holding `=`, is EINVAL.
  def __check_name__(name)
    raise Errno::EINVAL, "setenv(#{name})" if name.empty? || name.include?("=")
    name
  end

  def fetch(name, *default)
    __warning__("block supersedes default value argument") if !default.empty? && block_given?
    name = __name__(name)
    value = __lookup__(name)
    return value unless value.nil?
    return yield(name) if block_given?
    return default[0] unless default.empty?
    raise KeyError.new("key not found: #{name.inspect}", receiver: self, key: name)
  end

  def []=(name, value)
    name = __name__(name)
    if value.nil?
      __store__(name, nil)
    else
      value = __name__(value)
      __store__(__check_name__(name).dup.freeze, value.dup.freeze)
    end
    value
  end

  alias store []=

  def delete(name)
    name = __name__(name)
    value = __lookup__(name)
    __store__(name, nil)
    value.nil? && block_given? ? yield(name) : value
  end

  def key?(name)
    !__lookup__(__name__(name)).nil?
  end

  alias has_key? key?
  alias include? key?
  alias member? key?

  def value?(value)
    return nil unless String === value || value.respond_to?(:to_str)
    value = value.to_str unless String === value
    __table__.value?(value)
  end

  alias has_value? value?

  def keys = __table__.keys
  def values = __table__.values
  def size = __table__.size
  alias length size
  def empty? = __table__.empty?
  def to_h(&block) = block ? __table__.to_h(&block) : __table__.dup
  def to_hash = __table__.dup
  def to_a = __table__.to_a

  def each(&block)
    return enum_for(:each) { size } unless block
    __table__.each(&block)
    self
  end

  alias each_pair each

  def each_key(&block)
    return enum_for(:each_key) { size } unless block
    __table__.each_key(&block)
    self
  end

  def each_value(&block)
    return enum_for(:each_value) { size } unless block
    __table__.each_value(&block)
    self
  end

  def update(*others)
    others.each do |other|
      other.to_hash.each do |name, value|
        name = __name__(name)
        if block_given? && __table__.key?(name)
          value = yield(name, __table__[name], value)
        end
        self[name] = value
      end
    end
    self
  end

  def select(&block)
    return enum_for(:select) { size } unless block
    to_h.select(&block)
  end

  alias filter select

  def reject(&block)
    return enum_for(:reject) { size } unless block
    to_h.reject(&block)
  end

  def dup
    raise TypeError, "Cannot dup ENV, use ENV.to_h to get a copy of ENV as a hash"
  end

  def clone(freeze: nil)
    unless freeze.nil? || freeze == true || freeze == false
      raise ArgumentError, "unexpected value for freeze: #{freeze.class}"
    end
    raise TypeError, "Cannot clone ENV, use ENV.to_h to get a copy of ENV as a hash"
  end

  alias merge! update

  # All or nothing: every pair is checked before any is stored. Measured.
  def replace(other)
    pairs = other.to_hash.map do |name, value|
      [__check_name__(__name__(name)), __name__(value)]
    end
    __table__.clear
    pairs.each { |name, value| self[name] = value }
    self
  end

  def clear
    __table__.clear
    self
  end

  def inspect = __table__.inspect
  def to_s = "ENV"
end

# The process's identity and clocks (#145, for mspec), and since #43
# starting, waiting for and signalling other processes.
module Process
  __sys_clock_ids__.each { |name, id| const_set(name, id) }
  ARGV0 = $0.dup.freeze
  private_constant :ARGV0
  __sys_process_constants__.each { |name, value| const_set(name, value) }

  # `Process.exit` and friends are the Kernel functions on the module.
  def self.exit(status = true) = Kernel.exit(status)
  def self.exit!(status = false) = Kernel.exit!(status)
  def self.abort(*message) = Kernel.abort(*message)

  def self.pid = __sys_ids__[0]
  def self.ppid = __sys_ids__[1]
  def self.uid = __sys_ids__[2]
  def self.euid = __sys_ids__[3]
  def self.gid = __sys_ids__[4]
  def self.egid = __sys_ids__[5]

  def self.clock_gettime(clock, unit = :float_second)
    reading = __sys_clock__(clock)
    raise SystemCallError.new("clock_gettime(#{clock})", reading) if Integer === reading
    seconds, nanoseconds = reading
    case unit
    when :float_second then seconds + nanoseconds / 1_000_000_000.0
    when :float_millisecond then seconds * 1000.0 + nanoseconds / 1_000_000.0
    when :float_microsecond then seconds * 1_000_000.0 + nanoseconds / 1000.0
    when :second then seconds
    when :millisecond then seconds * 1000 + nanoseconds / 1_000_000
    when :microsecond then seconds * 1_000_000 + nanoseconds / 1000
    when :nanosecond then seconds * 1_000_000_000 + nanoseconds
    else raise ArgumentError, "unexpected unit: #{unit}"
    end
  end
end

# --- children (#43) ----------------------------------------------------------
#
# Starting one is `fork(2)` and `execve(2)` in one primitive, `__sys_spawn__`,
# handed everything already decided: the program found on `PATH`, the whole
# environment, and each descriptor the child gets as `[child, parent]`. What
# Ruby says about a command — a String that needs a shell, an Array that does
# not, the redirection options — is all here.

module Process
  # What `waitpid(2)` said about a child, kept as the raw status word. The
  # macros are POSIX's and read the same bits on every target Spinel builds
  # for.
  class Status
    class << self
      undef_method :new
    end

    def self.__make__(pid, status)
      object = allocate
      object.__set__(pid, status)
      object
    end

    def self.wait(pid = -1, flags = 0)
      result = __sys_waitpid__(Integer.__index__(pid), Integer.__index__(flags))
      return nil if Integer === result && result == Errno::ECHILD::Errno
      raise SystemCallError.new(nil, result) if Integer === result
      return nil if result[0] == 0
      # Measured: unlike `Process.wait`, this leaves `$?` alone.
      __make__(result[0], result[1])
    end

    def __set__(pid, status)
      @pid = pid
      @status = status
    end

    attr_reader :pid

    def to_i = @status

    def exited? = @status & 0x7f == 0
    def exitstatus = exited? ? (@status >> 8) & 0xff : nil
    def signaled? = @status & 0x7f != 0 && @status & 0x7f != 0x7f
    def termsig = signaled? ? @status & 0x7f : nil
    def stopped? = @status & 0xff == 0x7f
    def stopsig = stopped? ? (@status >> 8) & 0xff : nil
    def coredump? = signaled? && @status & 0x80 != 0
    def success? = exited? ? exitstatus == 0 : nil

    def ==(other)
      Status === other ? @status == other.to_i : @status == other
    end

    def &(other)
      __warning__("Process::Status#& is deprecated and will be removed in Ruby 3.5; use other Process::Status predicates instead")
      @status & Integer.__index__(other)
    end

    def >>(other)
      __warning__("Process::Status#>> is deprecated and will be removed in Ruby 3.5; use other Process::Status attributes instead")
      @status >> Integer.__index__(other)
    end

    def to_s
      "pid #{@pid} #{__state__}"
    end

    def inspect
      "#<Process::Status: pid #{@pid} #{__state__}>"
    end

    # CRuby's `pst_message_status`.
    def __state__
      if stopped?
        name = Signal.signame(stopsig)
        name.nil? ? "stopped signal #{stopsig}" : "stopped SIG#{name} (signal #{stopsig})"
      elsif signaled?
        name = Signal.signame(termsig)
        text = name.nil? ? "signal #{termsig}" : "SIG#{name} (signal #{termsig})"
        coredump? ? text + " (core dumped)" : text
      else
        "exit #{exitstatus}"
      end
    end
  end

  def self.last_status = $?

  def self.wait(pid = -1, flags = 0)
    status = __wait__(pid, flags)
    status&.pid
  end

  singleton_class.alias_method :waitpid, :wait

  def self.wait2(pid = -1, flags = 0)
    status = __wait__(pid, flags)
    status.nil? ? nil : [status.pid, status]
  end

  singleton_class.alias_method :waitpid2, :wait2

  def self.waitall
    all = []
    loop do
      result = __sys_waitpid__(-1, 0)
      break if Integer === result && result == Errno::ECHILD::Errno
      raise SystemCallError.new(nil, result) if Integer === result
      status = Status.__make__(result[0], result[1])
      __global_store__(:$?, status)
      all << [status.pid, status]
    end
    all
  end

  # `waitpid(2)`, raising `ECHILD` as `Process.wait` does and
  # `Process::Status.wait` does not. Nil when `WNOHANG` found nothing.
  def self.__wait__(pid, flags)
    result = __sys_waitpid__(Integer.__index__(pid), Integer.__index__(flags))
    raise SystemCallError.new(nil, result) if Integer === result
    return nil if result[0] == 0
    status = Status.__make__(result[0], result[1])
    __global_store__(:$?, status)
    status
  end

  # ponytail: CRuby answers a Thread that waits for the child, and there is
  # no `Thread` yet (#45).
  def self.detach(pid)
    Thread.new { Process.wait(pid) }
  end

  # A signal by number, by name with or without `SIG`, or as a Symbol; a
  # leading `-` signals the process group.
  def self.__signal_number__(signal)
    return [Integer.__index__(signal), false] if Integer === signal || !(String === signal || Symbol === signal || signal.respond_to?(:to_str))
    name = Symbol === signal ? signal.to_s : (String === signal ? signal : signal.to_str)
    group = name.start_with?("-")
    name = name[1..] if group
    bare = name.start_with?("SIG") ? name[3..] : name
    number = Signal.list[bare]
    raise ArgumentError, "unsupported signal 'SIG#{bare}'" if number.nil?
    [number, group]
  end

  def self.kill(signal, *pids)
    raise ArgumentError, "wrong number of arguments (given 1, expected 2+)" if pids.empty?
    number, group = __signal_number__(signal)
    if Integer === signal && number < 0
      number = -number
      group = true
    end
    pids.each do |pid|
      pid = Integer.__index__(pid)
      # ponytail: `Signal.trap` records a handler and nothing runs it, and the
      # signals CRuby turns into exceptions (INT, TERM and the other reserved
      # ones) have no exception to become. A signal this process sends itself
      # that Ruby would act on is refused until handlers run (#44).
      # One the operating system acts on alone — KILL, STOP, or any left
      # at its default — is sent.
      if number != 0 && Signal.__handler_for__(number) != "SYSTEM_DEFAULT" &&
         (pid == Process.pid || pid == 0 || (group && pid == Process.getpgrp) || pid == -Process.getpgrp)
        __needs_signals__
      end
      result = __sys_kill__(number, group ? -pid : pid)
      raise SystemCallError.new(nil, result) if Integer === result
    end
    pids.size
  end

  def self.fork(&block) = Kernel.fork(&block)

  # The fork itself, which `fork` calls so a program can hook it, as Ruby 3.1
  # made it: the pid, 0 in the child.
  def self._fork
    result = __sys_fork__
    raise SystemCallError.new("fork(2)", result) if Integer === result
    __global_store__(:$$, Process.pid) if result[0] == 0
    result[0]
  end

  # The program's name as it was given, whatever `$0` has been set to since.
  def self.argv0 = ARGV0

  def self.spawn(*args) = Kernel.spawn(*args)

  def self.exec(*args) = Kernel.exec(*args)

  def self.__pgid__(op, a = 0, b = 0)
    result = __sys_pgid__(op, a, b)
    raise SystemCallError.new(nil, result) if Integer === result
    result[0]
  end

  def self.getpgid(pid) = __pgid__(0, Integer.__index__(pid))
  def self.getpgrp = __pgid__(4)
  def self.getsid(pid = nil) = __pgid__(2, pid.nil? ? 0 : Integer.__index__(pid))
  def self.setsid = __pgid__(3)

  def self.setpgid(pid, group)
    __pgid__(1, Integer.__index__(pid), Integer.__index__(group))
    0
  end

  def self.setpgrp
    setpgid(0, 0)
    0
  end

  # What a command line is, decided once for `spawn`, `system`, `exec`,
  # backticks and `IO.popen`.
  module Spawn
    # A word that makes `/bin/sh` necessary: CRuby's `rb_exec_fillarg`.
    META = /[*?{}\[\]<>()~&|\\$;'`"\n#]/
    RESERVED = %w[case do done elif else esac fi for if in then until while ! { } [[ ]]].freeze
    BUILTINS = %w[: . break continue eval exec exit export readonly return set shift times trap unset].freeze

    module_function

    # `[env, argv0_and_args or shell line, options]` from `spawn`'s arguments.
    def split(args, name = "spawn")
      args = args.dup
      env = {}
      if Hash === args.first
        env = args.shift
      elsif !args.first.nil? && !(String === args.first) && !(Array === args.first) && args.first.respond_to?(:to_hash)
        env = args.shift.to_hash
      end
      options = Hash === args.last ? args.pop : {}
      raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if args.empty?
      [env, args, options]
    end

    # The program, its `argv` and whether a shell runs it.
    def command(args)
      # A first word that converts with `to_ary` is `[program, argv0]`.
      head = args[0]
      if !(String === head) && !(Array === head) && head.respond_to?(:to_ary)
        args = [head.to_ary] + args[1..]
      end
      if args.size == 1 && !(Array === args[0])
        line = File.__path_of__(args[0]) unless String === args[0]
        line ||= args[0]
        raise ArgumentError, "string contains null byte" if line.include?("\0")
        words = line.split(/[ \t]+/).reject(&:empty?)
        # Measured: nothing to run is no file, not an empty shell.
        raise Errno::ENOENT, line if words.empty?
        if line.match?(META) || words.empty? || RESERVED.include?(words[0]) || BUILTINS.include?(words[0]) || words[0].include?("=")
          return ["/bin/sh", ["sh", "-c", line], line]
        end
        return [find(words[0]), words, words[0]]
      end
      head = args[0]
      if Array === head
        raise ArgumentError, "wrong first argument" unless head.size == 2
        program = IO.__str__(head[0])
        argv0 = IO.__str__(head[1])
      else
        program = argv0 = IO.__str__(head)
      end
      rest = args[1..].map { |arg| IO.__str__(arg) }
      raise ArgumentError, "string contains null byte" if rest.any? { |arg| arg.include?("\0") } || program.include?("\0")
      [find(program), [argv0] + rest, program]
    end

    # `execvp(3)`'s search, done before the fork so its failure is an
    # exception in the parent.
    def find(program)
      raise Errno::ENOENT, program if program.empty?
      return program if program.include?("/")
      (ENV["PATH"] || "/usr/local/bin:/usr/bin:/bin").split(":").each do |dir|
        candidate = File.join(dir.empty? ? "." : dir, program)
        return candidate if File.file?(candidate) && File.executable?(candidate)
      end
      raise Errno::ENOENT, program
    end

    def environment(env, options)
      table = options[:unsetenv_others] ? {} : ENV.to_h
      env.each do |key, value|
        key = IO.__str__(key)
        raise ArgumentError, "environment name contains a equal : #{key}" if key.include?("=")
        value = IO.__str__(value) unless value.nil?
        raise ArgumentError, "string contains null byte" if key.include?("\0") || value&.include?("\0")
        value.nil? ? table.delete(key) : table[key] = value
      end
      table.map { |key, value| "#{key}=#{value}" }
    end

    # A redirection's left side as the child's descriptors.
    def target(key)
      case key
      when :in, STDIN then 0
      when :out, STDOUT then 1
      when :err, STDERR then 2
      when Integer then key
      when IO then key.fileno
      else nil
      end
    end

    OPTIONS = %i[pgroup new_pgroup unsetenv_others chdir umask close_others uid gid].freeze

    # CRuby's `rb_execarg_addopt`: what a key may be, and the two that must
    # be true or false.
    def check(options)
      options.each do |key, value|
        next if Integer === key || IO === key || Array === key || %i[in out err].include?(key)
        raise ArgumentError, "wrong exec option" unless Symbol === key
        # ponytail: `rlimit_*`, `:uid` and `:gid` are accepted and not applied;
        # each is one more call in the child before `execve`.
        next if key.to_s.start_with?("rlimit_")
        raise ArgumentError, "wrong exec option symbol: #{key}" unless OPTIONS.include?(key)
        if (key == :unsetenv_others || key == :close_others) && !(value.nil? || true.equal?(value) || false.equal?(value))
          raise ArgumentError, "expected true or false as #{key}: #{value.inspect}"
        end
      end
    end

    # The options as `[child, parent]` pairs, the files opened for them to be
    # closed after the fork, and the rest of the options.
    def redirections(options, extra = [])
      pairs = extra.dup
      opened = []
      children = []
      rest = {}
      options.each do |key, value|
        keys = Array === key ? key : [key]
        fds = keys.map { |one| target(one) }
        if fds.include?(nil)
          rest[key] = value
          next
        end
        # `[:out, :err] => name` opens the file once, for both.
        shared = nil
        fds.each do |fd|
          unless shared.nil?
            pairs << [fd, shared]
            next
          end
          source = case value
                   when :close then -1
                   when :in, :out, :err, Integer, IO then target(value)
                   when String
                     file = File.open(value, fd == 0 ? "r" : "w")
                     opened << file
                     file.fileno
                   when Array
                     if value[0] == :child
                       children << [fd, target(value[1])]
                       next
                     end
                     path, mode, perm = value
                     mode ||= "r"
                     file = File.open(File.__path__(path), mode, perm || 0644)
                     opened << file
                     file.fileno
                   else
                     path = File.__path__(value)
                     file = File.open(path, fd == 0 ? "r" : "w")
                     opened << file
                     file.fileno
                   end
          pairs << [fd, source]
          shared = source if fds.size > 1 && !(Array === value && value[0] == :child)
        end
      end
      # `[:child, fd]` is whatever that fd is in the child, so it is resolved
      # against the other redirections.
      children.each do |fd, other|
        made = pairs.find { |child, _| child == other }
        pairs << [fd, made.nil? ? other : made[1]]
      end
      [pairs, opened, rest]
    end

    # Start a child, or with `replace` become it. Answers the pid.
    def start(args, extra = [], replace: false, name: "spawn")
      env, command_args, options = split(args, name)
      check(options)
      begin
        path, argv, shown = command(command_args)
      rescue SystemCallError
        # CRuby finds out in the child it forked, which then exits 127.
        __global_store__(:$?, Process::Status.__make__(0, 127 << 8)) unless replace
        raise
      end
      pairs, opened, rest = redirections(options, extra)
      chdir = rest[:chdir].nil? ? nil : File.__path__(rest[:chdir])
      pgroup = case rest[:pgroup]
               when nil, false then nil
               when true then 0
               else Integer.__index__(rest[:pgroup])
               end
      raise ArgumentError, "negative process group ID : #{pgroup}" if !pgroup.nil? && pgroup < 0
      umask = rest[:umask].nil? ? nil : Integer.__index__(rest[:umask])
      result = __sys_spawn__(path, argv, environment(env, rest), pairs.flatten, chdir, pgroup, umask, replace, rest[:close_others])
      raise SystemCallError.new(shown, result) if Integer === result
      if result.size == 2
        # The child ran and could not become the program: CRuby's `$?` is
        # that child, exit 127.
        __global_store__(:$?, Process::Status.__make__(result[0], 127 << 8))
        raise SystemCallError.new(shown, result[1])
      end
      result[0]
    ensure
      opened&.each { |file| file.close unless file.closed? }
    end
  end
end

module Kernel
  def spawn(*args)
    Process::Spawn.start(args)
  end

  def exec(*args)
    Process::Spawn.start(args, replace: true, name: "exec")
  end

  # True when the command succeeded, false when it ran and failed, and nil
  # when it could not be started — unless `exception: true` asks for errors.
  def system(*args, exception: false, **options)
    args = args + [options] unless options.empty?
    begin
      pid = Process::Spawn.start(args, name: "system")
    rescue SystemCallError
      raise if exception
      __global_store__(:$?, Process::Status.__make__(0, 127 << 8))
      return nil
    end
    status = Process.__wait__(pid, 0)
    if exception && !status.success?
      _, command, = Process::Spawn.split(args)
      line = command.size == 1 && !(Array === command[0]) ? command[0] : command.flatten.join(" ")
      raise RuntimeError, "Command failed with #{status.__state__}: #{line}"
    end
    status.success?
  end

  # `` `cmd` `` compiles to this call (#223): the command's output, with `$?`
  # its status.
  def `(command)
    command = IO.__str__(command)
    IO.popen(command) { |io| io.read }
  end

  def exit!(status = false)
    __sys_exit__(Kernel.__exit_status__(status))
  end

  # With a block the child runs it and exits, running its `at_exit` blocks;
  # without one the child is answered nil.
  def fork
    pid = Process._fork
    return pid unless pid == 0
    return nil unless block_given?
    begin
      yield
    rescue SystemExit
      raise
    end
    exit(0)
  end

  module_function :spawn, :exec, :system, :`, :exit!, :fork
end

# --- limits, priorities, times and ids (#43) ---------------------------------

module Process
  Tms = Struct.new(:utime, :stime, :cutime, :cstime)

  def self.times = Tms.new(*__sys_times__)

  # A resource as a number: an Integer, or a name with or without `RLIMIT_`.
  def self.__resource__(resource)
    return Integer.__index__(resource) unless Symbol === resource || String === resource || resource.respond_to?(:to_str)
    name = Symbol === resource ? resource.to_s : IO.__str__(resource)
    name = name.delete_prefix("RLIMIT_")
    raise ArgumentError, "invalid resource name: #{name}" unless const_defined?("RLIMIT_#{name}", false)
    const_get("RLIMIT_#{name}", false)
  end

  # A limit as a number: an Integer, or `INFINITY`, `SAVED_MAX` or `SAVED_CUR`.
  def self.__limit__(limit)
    return Integer.__index__(limit) unless Symbol === limit || String === limit || limit.respond_to?(:to_str)
    name = Symbol === limit ? limit.to_s : IO.__str__(limit)
    name = name.delete_prefix("RLIM_")
    raise ArgumentError, "invalid resource value: #{name}" unless const_defined?("RLIM_#{name}", false)
    const_get("RLIM_#{name}", false)
  end

  def self.getrlimit(resource)
    result = __sys_rlimit__(__resource__(resource))
    raise SystemCallError.new("getrlimit", result) if Integer === result
    result
  end

  def self.setrlimit(resource, cur, max = cur)
    result = __sys_rlimit__(__resource__(resource), __limit__(cur), __limit__(max))
    raise SystemCallError.new("setrlimit", result) if Integer === result
    nil
  end

  def self.getpriority(kind, id)
    result = __sys_priority__(Integer.__index__(kind), Integer.__index__(id))
    raise SystemCallError.new(nil, result) if Integer === result
    result[0]
  end

  def self.setpriority(kind, id, priority)
    result = __sys_priority__(Integer.__index__(kind), Integer.__index__(id), Integer.__index__(priority))
    raise SystemCallError.new(nil, result) if Integer === result
    0
  end

  # The emulated clocks CRuby names, with the resolutions it answers for them.
  EMULATED_RESOLUTIONS = {
    GETTIMEOFDAY_BASED_CLOCK_REALTIME: 1_000,
    TIME_BASED_CLOCK_REALTIME: 1_000_000_000,
    GETRUSAGE_BASED_CLOCK_PROCESS_CPUTIME_ID: 1_000,
    TIMES_BASED_CLOCK_PROCESS_CPUTIME_ID: 10_000_000,
    TIMES_BASED_CLOCK_MONOTONIC: 10_000_000,
    CLOCK_BASED_CLOCK_PROCESS_CPUTIME_ID: 1_000,
  }.freeze
  private_constant :EMULATED_RESOLUTIONS

  def self.clock_getres(clock, unit = :float_second)
    if Symbol === clock
      nanoseconds = EMULATED_RESOLUTIONS.fetch(clock) { raise Errno::EINVAL, "clock_getres(#{clock})" }
      seconds = 0
    else
      reading = __sys_clock_res__(Integer.__index__(clock))
      raise SystemCallError.new("clock_getres(#{clock})", reading) if Integer === reading
      seconds, nanoseconds = reading
    end
    total = seconds * 1_000_000_000 + nanoseconds
    case unit
    when :float_second then total / 1_000_000_000.0
    when :float_millisecond then total / 1_000_000.0
    when :float_microsecond then total / 1000.0
    when :second then total / 1_000_000_000
    when :millisecond then total / 1_000_000
    when :microsecond then total / 1000
    when :nanosecond then total
    else raise ArgumentError, "unexpected unit: #{unit}"
    end
  end

  def self.groups = __sys_groups__[0...-1]
  def self.maxgroups = __sys_groups__.last

  def self.__set_id__(op, a, b = -1)
    result = __sys_set_id__(op, Integer.__index__(a), Integer.__index__(b))
    raise SystemCallError.new(nil, result) if Integer === result
    nil
  end

  def self.uid=(id)
    __set_id__(0, id)
    id
  end

  def self.gid=(id)
    __set_id__(1, id)
    id
  end

  def self.euid=(id)
    __set_id__(2, id)
    id
  end

  def self.egid=(id)
    __set_id__(3, id)
    id
  end

  # The C calls by their own names.
  module Sys
    def self.getuid = Process.uid
    def self.geteuid = Process.euid
    def self.getgid = Process.gid
    def self.getegid = Process.egid
    def self.setuid(id) = Process.__set_id__(0, id)
    def self.setgid(id) = Process.__set_id__(1, id)
    def self.seteuid(id) = Process.__set_id__(2, id)
    def self.setegid(id) = Process.__set_id__(3, id)
    def self.setreuid(real, effective) = Process.__set_id__(4, real, effective)
    def self.setregid(real, effective) = Process.__set_id__(5, real, effective)
    def self.setruid(id) = Process.__set_id__(4, id, -1)
    def self.setrgid(id) = Process.__set_id__(5, id, -1)
    def self.issetugid = false
  end

  # The real and effective user id, and the same for groups, as CRuby's
  # `Process::UID` and `Process::GID` present them.
  module UID
    def self.rid = Process.uid
    def self.eid = Process.euid

    def self.eid=(id)
      Process.euid = id
    end

    def self.change_privilege(id)
      Process.__set_id__(4, id, id)
      id
    end

    def self.grant_privilege(id)
      Process.euid = id
      id
    end

    def self.re_exchangeable? = true

    def self.re_exchange
      real, effective = rid, eid
      Process.__set_id__(4, effective, real)
      real
    end

    def self.sid_available? = true

    def self.switch
      real, effective = rid, eid
      Process.__set_id__(4, effective, real)
      return effective unless block_given?
      begin
        yield
      ensure
        Process.__set_id__(4, real, effective)
      end
    end

    def self.from_name(name)
      entry = File.foreach("/etc/passwd").map { |line| line.split(":") }.find { |fields| fields[0] == IO.__str__(name) }
      raise ArgumentError, "can't find user for #{name}" if entry.nil?
      entry[2].to_i
    end
  end

  module GID
    def self.rid = Process.gid
    def self.eid = Process.egid

    def self.eid=(id)
      Process.egid = id
    end

    def self.change_privilege(id)
      Process.__set_id__(5, id, id)
      id
    end

    def self.grant_privilege(id)
      Process.egid = id
      id
    end

    def self.re_exchangeable? = true

    def self.re_exchange
      real, effective = rid, eid
      Process.__set_id__(5, effective, real)
      real
    end

    def self.sid_available? = true

    def self.switch
      real, effective = rid, eid
      Process.__set_id__(5, effective, real)
      return effective unless block_given?
      begin
        yield
      ensure
        Process.__set_id__(5, real, effective)
      end
    end

    def self.from_name(name)
      entry = File.foreach("/etc/group").map { |line| line.split(":") }.find { |fields| fields[0] == IO.__str__(name) }
      raise ArgumentError, "can't find group for #{name}" if entry.nil?
      entry[2].to_i
    end
  end

  # `daemon(3)`: the parent ends, and the child goes on in a session of its
  # own, at `/` and with its standard streams on `/dev/null` unless asked
  # otherwise. Measured: the `at_exit` blocks of the parent do not run.
  def self.daemon(nochdir = nil, noclose = nil)
    { nochdir: nochdir, noclose: noclose }.each do |name, value|
      next if value.nil? || true.equal?(value) || false.equal?(value)
      raise ArgumentError, "expected true or false as #{name}: #{value.inspect}"
    end
    pid = _fork
    __sys_exit__(0) unless pid == 0
    setsid
    Dir.chdir("/") unless nochdir
    unless noclose
      null = File.open(File::NULL, "r+")
      [0, 1, 2].each { |fd| __fd_dup2__(null.fileno, fd) }
      null.close
    end
    0
  end
end
