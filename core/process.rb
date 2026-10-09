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

module Kernel
  # `` `cmd` `` compiles to this call (#223); running the command is a child
  # process, which is `Process` (#43).
  def `(command)
    __needs_process__
  end

  module_function :`
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

# The process's identity and clocks (#145, for mspec). Starting, waiting for
# and signalling other processes is the rest of `Process`, which is #43.
module Process
  __sys_clock_ids__.each { |name, id| const_set(name, id) }
  __sys_process_constants__.each { |name, value| const_set(name, value) }

  # `Process.exit` and friends are the Kernel functions on the module.
  def self.exit(status = true) = Kernel.exit(status)
  def self.exit!(status = false) = Kernel.exit!(status)
  def self.abort(*message) = Kernel.abort(*message)
  def self.fork(...) = __needs_process__
  def self._fork = __needs_process__

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
