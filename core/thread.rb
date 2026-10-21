# Thread, Mutex, ConditionVariable, Queue, SizedQueue and ThreadGroup (#45).
#
# The scheduler is the VM's, in `spinel-vm/src/threads.rs`: a thread is a
# saved set of the interpreter's vectors, and `__thread_block__` waits —
# until a deadline, a descriptor, or another thread's `__thread_wakeup__` —
# while another runs. Every rule above that is here: what `join` waits for,
# who holds a `Mutex`, what a `Queue` hands out, how a thread reports the
# exception it died of. `__thread_block__` answers why it woke: `:timeout`,
# `:ready`, `:woken`, `:interrupted`, or `:deadlock` for the main thread when
# every thread waits on another.
#
# ponytail: the threads of a Ractor take turns on one OS thread (see
# `threads.rs`); `Thread#[]` is per thread rather than per fiber; and
# `handle_interrupt` defers nothing.
class Thread
  # The `fatal` CRuby raises in the main thread on a deadlock: an Exception
  # no `rescue => e` catches, and no constant names.
  FATAL = Class.new(Exception) do
    def self.name = "fatal"
    def self.to_s = "fatal"
    def self.inspect = "fatal"
  end
  private_constant :FATAL

  @class_report_on_exception = true
  @class_abort_on_exception = false
  @all = []

  # What a thread knows of itself, apart from it so that freezing a `Thread`
  # freezes only what its own methods would change.
  class State
    attr_accessor :args, :block, :locals, :variables, :joiners, :mutexes, :report_on_exception,
                  :abort_on_exception, :priority, :name, :group, :exception, :value, :dead, :killing,
                  :location,
                  :started, :deferred, :masks, :pending, :storage, :recursive
  end
  private_constant :State

  # Measured: CRuby's `Thread` has no allocator.
  def self.allocate
    Kernel.raise TypeError, "allocator undefined for Thread"
  end

  def self.__allocate__ = Class.instance_method(:allocate).bind_call(self)

  def self.new(*args, **options, &block)
    thread = __allocate__
    if options.empty?
      thread.__send__(:initialize, *args, &block)
    else
      thread.__send__(:initialize, *args, **options, &block)
    end
    unless thread.__started__?
      Kernel.raise ThreadError, "uninitialized thread - check `#{self}#initialize'"
    end
    thread
  end

  def self.start(*args, &block)
    Kernel.raise ThreadError, "must be called with a block" if block.nil?
    thread = __allocate__
    thread.__start__(args, block)
    thread
  end

  def initialize(*args, &block)
    Kernel.raise ThreadError, "must be called with a block" if block.nil?
    Kernel.raise ThreadError, "already initialized thread" if __started__?
    __start__(args, block)
  end

  def self.current
    index = __thread_current__
    __thread_object__(index) || (index == 0 ? main : nil)
  end

  def self.main
    __thread_object__(0) || begin
      thread = __allocate__
      thread.__setup__(nil, nil)
      thread.__index__ = 0
      thread.__started__!
      __thread_set_object__(0, thread)
      thread
    end
  end

  def self.list
    main
    @all.reject! { |t| !t.alive? }
    [main] + (@all - [main])
  end

  # In a child after `fork`: only the thread that forked survives.
  def self.__forked__
    me = current
    @all.each { |t| t.__died__ unless t.equal?(me) }
    main.__died__ unless main.equal?(me)
    __thread_forked__
  end

  def __died__
    @__s.dead = true
  end

  def self.__register__(thread)
    @all.reject! { |t| !t.alive? }
    @all << thread
  end

  def self.pass
    __thread_pass__
    nil
  end

  def self.stop
    current.__sleep__(nil, false)
    nil
  end

  def self.exit = current.kill
  def self.kill(thread) = thread.kill

  def self.abort_on_exception = @class_abort_on_exception
  def self.abort_on_exception=(value)
    @class_abort_on_exception = value ? true : false
  end

  def self.report_on_exception = @class_report_on_exception
  def self.report_on_exception=(value)
    @class_report_on_exception = value ? true : false
  end

  # Interrupts — another thread's `Thread#raise` — held back as the masks
  # say: `:never` until the block ends, `:on_blocking` until the thread waits
  # for something, `:immediate` not at all. Held ones are raised when a mask
  # lets them through, the first first.
  MODES = %i[immediate on_blocking never].freeze
  private_constant :MODES

  def self.handle_interrupt(hash)
    unless Hash === hash && !hash.empty? && hash.values.all? { |mode| MODES.include?(mode) }
      Kernel.raise ArgumentError, "unknown mask signature"
    end
    Kernel.raise ArgumentError, "block is needed." unless block_given?
    me = current
    me.__masks__.push(hash)
    begin
      me.__deliver__(false)
      yield
    ensure
      me.__masks__.pop
      me.__deliver__(false)
    end
  end

  def self.pending_interrupt?(error = nil) = current.pending_interrupt?(error)

  def pending_interrupt?(error = nil)
    pending = __pending__
    error.nil? ? !pending.empty? : pending.any? { |e| error === e }
  end

  def __masks__ = (@__s.masks ||= [])
  def __pending__ = (@__s.pending ||= [])

  # What the masks say for `error`, innermost first.
  def __mode__(error)
    __masks__.reverse_each do |mask|
      mask.each { |klass, mode| return mode if klass === error }
    end
    :immediate
  end

  # Raise the first held interrupt the masks let through now; `blocking` is
  # the thread about to wait.
  def __deliver__(blocking)
    pending = __pending__
    return if pending.empty?
    index = pending.index do |error|
      mode = __mode__(error)
      mode == :immediate || (blocking && mode == :on_blocking)
    end
    Kernel.raise pending.delete_at(index) unless index.nil?
  end

  # Called by every wait before it waits: `:on_blocking` interrupts go now.
  def self.__before_wait__
    me = __thread_object__(__thread_current__)
    me&.__deliver__(true)
  end

  # A thread every thread waits on, with nothing left that could end it.
  def self.__deadlock__
    count = list.size
    Kernel.raise FATAL, "No live threads left. Deadlock?\n#{count} threads, #{count} sleeps current:#{current.__thread_address__} main thread:#{main.__thread_address__}"
  end

  # --- one thread ----------------------------------------------------------------

  def __setup__(args, block)
    @__s = State.new
    @__s.args = args
    @__s.block = block
    @__s.locals = nil
    @__s.variables = {}
    @__s.joiners = []
    @__s.mutexes = []
    @__s.report_on_exception = Thread.report_on_exception
    @__s.abort_on_exception = false
    @__s.priority = 0
    @__s.name = nil
    @__s.group = Thread.__default_group__
    @__s.exception = nil
    @__s.value = nil
    @__s.dead = false
  end

  def __started__? = !@index.nil?
  def __started__! = (@__s.started = true)
  def __index__ = @index
  def __index__=(index)
    @index = index
  end

  def __mutexes__ = @__s.mutexes

  def __start__(args, block)
    if !Thread.current.group.nil?
      group = Thread.current.group
    end
    __setup__(args, block)
    @__s.group = group || Thread.__default_group__
    @__s.location = __proc_location__(block)
    @__s.storage = Fiber.current.__storage_copy__
    @index = __thread_new__(self, -> { __run__ })
    Thread.__register__(self)
    self
  end

  # The bottom of every thread but the main one: the block, what it raised,
  # and the end — which wakes whoever joins it and never returns.
  def __run__
    catch(self) do
      @__s.started = true
      Fiber.current.__set_storage__(@__s.storage)
      # What was asked of it before it ran, now that it can be.
      Kernel.throw self if @__s.killing
      Kernel.raise @__s.deferred unless @__s.deferred.nil?
      @__s.value = @__s.block.call(*@__s.args)
    rescue Exception => e
      @__s.exception = e
      # An `exit` from any thread ends the program: the main thread gets it.
      if SystemExit === e
        Thread.main.raise(e)
        next
      end
      __report__(e) if @__s.report_on_exception
      if @__s.abort_on_exception || Thread.abort_on_exception
        Thread.main.raise(e)
      end
    end
  ensure
    @__s.dead = true
    @__s.mutexes.dup.each { |m| m.__abandon__ }
    @__s.joiners.each { |index| __thread_wakeup__(index) }
    @__s.joiners.clear
    __thread_exit__
  end

  def __report__(error)
    message = "#{inspect} terminated with exception (report_on_exception is true):\n#{error.full_message(highlight: false)}"
    $stderr.write(message)
  rescue Exception
    nil
  end

  def join(limit = nil)
    unless limit.nil?
      limit = Float === limit || Integer === limit ? limit : Float(limit)
    end
    Kernel.raise ThreadError, "Target thread must not be current thread" if equal?(Thread.current)
    Kernel.raise ThreadError, "Target thread must not be main thread" if @index == 0
    deadline = limit && Process.clock_gettime(Process::CLOCK_MONOTONIC) + limit
    until @__s.dead
      me = __thread_current__
      @__s.joiners << me
      remaining = deadline && [deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC), 0].max
      begin
        Thread.__before_wait__
        reason = __thread_block__(remaining, nil, 0, true)
      ensure
        @__s.joiners.delete(me)
      end
      Thread.__deadlock__ if reason == :deadlock
      return nil if !@__s.dead && deadline && Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    end
    Kernel.raise @__s.exception unless @__s.exception.nil?
    self
  end

  def value
    join
    @__s.value
  end

  def alive? = !@__s.dead
  def stop? = @__s.dead || __thread_status__(@index) == :sleep

  def status
    return (@__s.exception.nil? ? false : nil) if @__s.dead
    # A dying thread is "aborting" while it runs and "sleep" while it
    # waits, measured.
    return "sleep" if __thread_status__(@index) == :sleep
    @__s.killing ? "aborting" : "run"
  end

  def kill
    return self if @__s.dead
    if @index == 0
      exit
    elsif equal?(Thread.current)
      @__s.killing = true
      throw self
    else
      @__s.killing = true
      __thread_interrupt__(@index, nil) if @__s.started
    end
    self
  end
  alias terminate kill
  alias exit kill

  def raise(*args, **options)
    return nil if @__s.dead
    if args.empty? && options.key?(:cause)
      Kernel.raise ArgumentError, "only cause is given with no arguments"
    end
    cause = options[:cause]
    unless cause.nil? || Exception === cause
      Kernel.raise TypeError, "exception object expected"
    end
    error = if args.empty?
              RuntimeError.new("unhandled exception")
            else
              # Built as `raise` builds it, without raising it here.
              begin
                Kernel.raise(*args, **options)
              rescue Exception => e
                e
              end
            end
    if equal?(Thread.current)
      Kernel.raise(error)
    elsif !@__s.started
      @__s.deferred = error
    else
      mode = __mode__(error)
      if mode == :never || (mode == :on_blocking && __thread_status__(@index) != :sleep)
        __pending__ << error
      else
        __thread_interrupt__(@index, error)
      end
    end
    nil
  end

  def wakeup
    Kernel.raise ThreadError, "killed thread" if @__s.dead
    __thread_wakeup__(@index)
    self
  end

  def run
    wakeup
    Thread.pass
    self
  end

  # `sleep` in this thread: until woken, or for `seconds`.
  def __sleep__(seconds, deadlockable)
    __thread_block__(seconds, nil, 0, deadlockable)
  end

  # --- locals ----------------------------------------------------------------

  def self.__key__(key)
    return key if Symbol === key
    return key.to_sym if String === key
    if key.respond_to?(:to_str)
      converted = key.to_str
      return converted.to_sym if String === converted
    end
    Kernel.raise TypeError, "#{key.inspect} is not a symbol nor a string"
  end

  def __locals__
    all = (@__s.locals ||= {}.compare_by_identity)
    fiber = equal?(Thread.current) ? Fiber.current : nil
    key = fiber.nil? || fiber.__root__? ? :root : fiber
    all[key] ||= {}
  end

  # CRuby's `rb_exec_recursive` bookkeeping, per fiber like the locals but
  # under no key a program can see. Only ever asked of `Thread.current`.
  def __recursive__
    all = (@__s.recursive ||= {}.compare_by_identity)
    fiber = Fiber.current
    all[fiber.__root__? ? :root : fiber] ||= {}
  end

  def [](key) = __locals__[Thread.__key__(key)]

  def []=(key, value)
    Kernel.raise FrozenError, "can't modify frozen thread locals" if frozen?
    key = Thread.__key__(key)
    if value.nil?
      __locals__.delete(key)
    else
      __locals__[key] = value
    end
  end

  def key?(key) = __locals__.key?(Thread.__key__(key))
  def keys = __locals__.keys

  def fetch(*args, &block)
    unless (1..2).cover?(args.size)
      Kernel.raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
    end
    key = Thread.__key__(args[0])
    warn "warning: block supersedes default value argument", uplevel: 1 if block && args.size == 2
    return __locals__[key] if __locals__.key?(key)
    return yield(key) if block
    return args[1] if args.size == 2
    Kernel.raise KeyError.new("key not found: #{key.inspect}", receiver: self, key: key)
  end

  def thread_variable_get(key) = @__s.variables[Thread.__key__(key)]

  def thread_variable_set(key, value)
    Kernel.raise FrozenError, "can't modify frozen thread locals" if frozen?
    key = Thread.__key__(key)
    if value.nil?
      @__s.variables.delete(key)
    else
      @__s.variables[key] = value
    end
    value
  end

  def thread_variable?(key) = @__s.variables.key?(Thread.__key__(key))
  def thread_variables = @__s.variables.keys

  # --- the rest --------------------------------------------------------------

  def name = @__s.name

  def name=(name)
    unless name.nil?
      name = IO.__str__(name)
      Kernel.raise ArgumentError, "string contains null byte" if name.include?("\0")
      name = name.dup.freeze
    end
    @__s.name = name
  end

  def priority = @__s.priority

  def priority=(value)
    @__s.priority = Integer.__index__(value).clamp(-3, 3)
  end

  def group = @__s.dead ? nil : @__s.group

  def __group__=(group)
    @__s.group = group
  end

  def abort_on_exception = @__s.abort_on_exception
  def abort_on_exception=(value)
    @__s.abort_on_exception = value ? true : false
  end

  def report_on_exception = @__s.report_on_exception
  def report_on_exception=(value)
    @__s.report_on_exception = value ? true : false
  end

  def backtrace(*args)
    return nil if @__s.dead
    # Its own frame first, as CRuby's, so `caller`'s arguments mean the same.
    return(args.empty? ? caller(0) : caller(*args)) if equal?(Thread.current)
    saved = __thread_backtrace__(@index)
    lines = saved.nil? ? [] : saved[0]
    args.empty? ? lines : Thread.__slice_backtrace__(lines, *args)
  end

  def backtrace_locations(*args)
    return nil if @__s.dead
    return(args.empty? ? caller_locations(0) : caller_locations(*args)) if equal?(Thread.current)
    saved = __thread_backtrace__(@index)
    lines = saved.nil? ? [] : saved[1].map { |path, line, label| Backtrace::Location.__from__(path, line, label) }
    args.empty? ? lines : Thread.__slice_backtrace__(lines, *args)
  end

  # `caller`'s `start, length` or range, on a backtrace already taken.
  def self.__slice_backtrace__(lines, start = 0, length = nil)
    if Range === start
      return lines[start]
    end
    start = Integer.__index__(start)
    Kernel.raise ArgumentError, "negative level (#{start})" if start < 0
    return nil if start > lines.size
    return lines[start..] if length.nil?
    length = Integer.__index__(length)
    Kernel.raise ArgumentError, "negative size (#{length})" if length < 0
    lines[start, length]
  end

  def native_thread_id = @__s.dead ? nil : Process.pid

  def __thread_address__ = "0x" + __address__

  def inspect
    out = "#<#{self.class}:#{__thread_address__}"
    out << "@#{@__s.name}" unless @__s.name.nil?
    out << " #{@__s.location[0]}:#{@__s.location[1]}" if @__s.location
    state = status
    out << " " << (state ? state : "dead") << ">"
    out.force_encoding(Encoding::BINARY)
  end
  alias to_s inspect

  def self.__default_group__ = ThreadGroup::Default

  class Mutex
    def initialize
      @owner = nil
      @waiters = []
    end

    def lock
      me = Thread.current
      fiber = Fiber.current
      Kernel.raise ThreadError, "deadlock; recursive locking" if @fiber.equal?(fiber)
      if @owner.equal?(me)
        Kernel.raise ThreadError, "deadlock; lock already owned by another fiber belonging to the same thread"
      end
      until @owner.nil?
        index = __thread_current__
        @waiters << index
        begin
          Thread.__before_wait__
          reason = __thread_block__(nil, nil, 0, true)
        ensure
          @waiters.delete(index)
        end
        Thread.__deadlock__ if reason == :deadlock
      end
      @owner = me
      @fiber = fiber
      me.__mutexes__ << self
      self
    end

    def try_lock
      return false unless @owner.nil?
      @owner = Thread.current
      @fiber = Fiber.current
      @owner.__mutexes__ << self
      true
    end

    def unlock
      Kernel.raise ThreadError, "Attempt to unlock a mutex which is not locked" if @owner.nil?
      unless @fiber.equal?(Fiber.current)
        Kernel.raise ThreadError, "Attempt to unlock a mutex which is locked by another thread/fiber"
      end
      __release__
      self
    end

    def __release__
      @owner.__mutexes__.delete(self)
      @owner = nil
      @fiber = nil
      __thread_wakeup__(@waiters.first) unless @waiters.empty?
    end

    # The owner died holding it: CRuby hands it on.
    def __abandon__ = __release__

    def locked? = !@owner.nil?
    def owned? = !@fiber.nil? && @fiber.equal?(Fiber.current)

    def synchronize
      Kernel.raise ThreadError, "must be called with a block" unless block_given?
      lock
      begin
        yield
      ensure
        unlock if owned?
      end
    end

    def sleep(timeout = nil)
      unless timeout.nil?
        unless Integer === timeout || Float === timeout || timeout.respond_to?(:divmod)
          Kernel.raise TypeError, "can't convert #{timeout.class} into time interval"
        end
        Kernel.raise ArgumentError, "time interval must not be negative" if timeout < 0
      end
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      unlock
      begin
        Thread.__before_wait__
        reason = __thread_block__(timeout, nil, 0, false)
      ensure
        lock
      end
      (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round
    end

    def marshal_dump
      Kernel.raise TypeError, "no _dump_data is defined for class #{self.class}"
    end
  end

  class ConditionVariable
    def initialize
      @waiters = []
    end

    def wait(mutex, timeout = nil)
      index = __thread_current__
      @waiters << index
      begin
        mutex.sleep(timeout)
      ensure
        @waiters.delete(index)
      end
      self
    end

    def signal
      __thread_wakeup__(@waiters.shift) unless @waiters.empty?
      self
    end

    def broadcast
      waiters = @waiters.dup
      @waiters.clear
      waiters.each { |index| __thread_wakeup__(index) }
      self
    end

    def marshal_dump
      Kernel.raise TypeError, "can't dump #{self.class}"
    end
  end

  class Queue
    def initialize(items = nil)
      @items = []
      unless items.nil?
        unless items.respond_to?(:to_a)
          Kernel.raise TypeError, "can't convert #{items.class} into Array"
        end
        array = items.to_a
        unless Array === array
          Kernel.raise TypeError, "can't convert #{items.class} to Array (#{items.class}#to_a gives #{array.class})"
        end
        @items.concat(array)
      end
      @waiters = []
      @closed = false
    end

    def push(object)
      Kernel.raise ClosedQueueError, "queue closed" if @closed
      @items.push(object)
      __thread_wakeup__(@waiters.first) unless @waiters.empty?
      self
    end
    alias << push
    alias enq push

    # Wait for the queue to change, until `deadline`: false when it passed.
    def __wait__(waiters, deadline)
      index = __thread_current__
      waiters << index
      remaining = deadline && [deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC), 0].max
      begin
        Thread.__before_wait__
        reason = __thread_block__(remaining, nil, 0, true)
      ensure
        waiters.delete(index)
      end
      Thread.__deadlock__ if reason == :deadlock
      !(deadline && Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline)
    end

    def __deadline__(non_block, timeout)
      if non_block && !timeout.nil?
        Kernel.raise ArgumentError, "can't set a timeout if non_block is enabled"
      end
      timeout && Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    end

    def pop(non_block = false, timeout: nil)
      deadline = __deadline__(non_block, timeout)
      while true
        unless @items.empty?
          item = @items.shift
          __popped__
          return item
        end
        Kernel.raise ThreadError, "queue empty" if non_block
        return nil if @closed
        return nil if timeout == 0
        return nil unless __wait__(@waiters, deadline)
      end
    end
    alias shift pop
    alias deq pop

    # A pop made room: `SizedQueue`'s pushers care.
    def __popped__; end

    def close
      unless @closed
        @closed = true
        @waiters.dup.each { |index| __thread_wakeup__(index) }
      end
      self
    end

    def closed? = @closed
    def empty? = @items.empty?
    def size = @items.size
    alias length size

    def clear
      @items.clear
      __popped__
      self
    end

    def num_waiting = @waiters.size

    def freeze
      Kernel.raise TypeError, "cannot freeze #{inspect}"
    end

    def marshal_dump
      Kernel.raise TypeError, "can't dump #{self.class}"
    end
  end

  class SizedQueue < Queue
    def initialize(max)
      max = Integer.__index__(max)
      Kernel.raise ArgumentError, "queue size must be positive" unless max > 0
      super()
      @max = max
      @pushers = []
    end

    attr_reader :max

    def max=(max)
      max = Integer.__index__(max)
      Kernel.raise ArgumentError, "queue size must be positive" unless max > 0
      @max = max
      @pushers.dup.each { |index| __thread_wakeup__(index) }
    end

    def push(object, non_block = false, timeout: nil)
      deadline = __deadline__(non_block, timeout)
      while true
        Kernel.raise ClosedQueueError, "queue closed" if @closed
        if @items.size < @max
          return super(object)
        end
        Kernel.raise ThreadError, "queue full" if non_block
        return nil if timeout == 0
        return nil unless __wait__(@pushers, deadline)
      end
    end
    alias << push
    alias enq push

    def __popped__
      __thread_wakeup__(@pushers.first) unless @pushers.empty?
    end

    def close
      super
      @pushers.dup.each { |index| __thread_wakeup__(index) }
      self
    end

    def num_waiting = @waiters.size + @pushers.size
  end

end

class ThreadGroup
  def initialize
    @enclosed = false
  end

  Default = new

  def add(thread)
    if frozen?
      Kernel.raise ThreadError, "can't move to the frozen thread group"
    end
    if @enclosed
      Kernel.raise ThreadError, "can't move to the enclosed thread group"
    end
    from = thread.group
    if from&.frozen?
      Kernel.raise ThreadError, "can't move from the frozen thread group"
    end
    if from&.enclosed?
      Kernel.raise ThreadError, "can't move from the enclosed thread group"
    end
    thread.__group__ = self
    self
  end

  def list = Thread.list.select { |t| t.group.equal?(self) }

  def enclose
    @enclosed = true
    self
  end

  def enclosed? = @enclosed
end

# `Process.detach`'s thread, waiting for the child and answering its status;
# `pid` names the child. Here because `core/process.rb` loads before `Thread`.
module Process
  class Waiter < Thread
    def pid = @__pid__

    def __pid__=(pid)
      @__pid__ = pid
    end
  end
end

Mutex = Thread::Mutex
ConditionVariable = Thread::ConditionVariable
Queue = Thread::Queue
SizedQueue = Thread::SizedQueue

class Thread
  module Backtrace
    # `--backtrace-limit`, or -1 for none.
    def self.limit = Exception.__backtrace_limit__ || -1

    # One frame of a backtrace. Built by `core/exception.rb` and `Kernel` from
    # the `[path, line, label]` triples the VM records, never by a program:
    # CRuby gives this class no allocator.
    class Location
      def self.__from__(path, lineno, label)
        location = allocate
        location.__send__(:__init_location__, path, lineno, label)
        location
      end

      def __init_location__(path, lineno, label)
        @path = path
        @lineno = lineno
        @label = label
      end

      def path
        @path
      end

      def lineno
        @lineno
      end

      # `Foo#bar`, `Foo.bar`, `block in Foo#bar`, `<main>` — CRuby 3.4's label.
      def label
        @label
      end

      # The method's own name, without the owner or any `block in`: `bar` for
      # all three of `Foo#bar`, `Foo.bar` and `block (2 levels) in Foo#bar`.
      # A label that is not a method — `<main>`, `<class:Foo>` — is its own.
      def base_label
        name = @label
        while name.start_with?("block ")
          at = name.index(" in ")
          break if at.nil?
          name = name[at + 4, name.length - at - 4]
        end
        return name if name.start_with?("<")
        cut = nil
        i = name.length - 1
        while i >= 0
          if name[i] == "#" || name[i] == "."
            cut = i
            break
          end
          i -= 1
        end
        cut.nil? ? name : name[cut + 1, name.length - cut - 1]
      end

      # The file resolved to an absolute path, or nil for one that is not a
      # file — `<internal:array>`, or code from `eval`.
      def absolute_path
        return nil if @path.start_with?("<internal:")
        __absolute_path__(@path)
      end

      def to_s
        "#{@path}:#{@lineno}:in '#{@label}'"
      end

      def inspect
        to_s.inspect
      end
    end
  end
end
