# Data — an immutable value made of named members.
#
# `Data.define(:x, :y)` answers a class whose instances hold one value per
# member, are frozen from birth, and have readers and no writers. It is
# `Struct`'s younger sibling and is written the same way, in Ruby: values are
# a Hash in a hidden instance variable, members an Array on the class.
#
# The protocol is in two steps, and a subclass may stand in the middle of it.
# `new` turns positional arguments into keywords; `initialize` takes only
# keywords, checks them and freezes. A class that overrides `initialize` sees
# keywords whichever way the caller wrote them.
class Data
  class << self
    undef_method :new
  end

  def self.define(*given, &block)
    members = given.map do |member|
      unless member.is_a?(Symbol) || member.is_a?(String)
        raise TypeError, member.inspect + " is not a symbol nor a string"
      end
      member = member.to_sym
      if member.to_s.end_with?("=")
        raise ArgumentError, "invalid data member: " + member.to_s
      end
      member
    end
    members.each_with_index do |member, index|
      if members.index(member) != index
        raise ArgumentError, "duplicate member: " + member.to_s
      end
    end

    data = Class.new(self)
    data.__define_data__(members)
    data.class_eval(&block) unless block.nil?
    data
  end

  def self.__define_data__(members)
    @__members__ = members
    members.each do |member|
      define_method(member) { __data_values__[member] }
    end
    class << self
      # Positional arguments are named by the members, in order, and the
      # object is then made from keywords alone.
      def new(*args, **keywords, &block)
        members = __members__
        unless args.empty?
          unless keywords.empty?
            raise ArgumentError,
                  "wrong number of arguments (given " + (args.size + 1).to_s + ", expected 0)"
          end
          if args.size > members.size
            raise ArgumentError, "wrong number of arguments (given " + args.size.to_s +
                                 ", expected 0.." + members.size.to_s + ")"
          end
          args.each_with_index { |value, index| keywords[members[index]] = value }
        end
        instance = allocate
        instance.__send__(:initialize, **keywords, &block)
        instance
      end
      alias [] new

      def members
        __members__.dup
      end
    end
  end

  def self.__members__
    scope = self
    while scope.respond_to?(:__own_members__)
      own = scope.__own_members__
      return own unless own.nil?
      scope = scope.superclass
    end
    raise TypeError, "uninitialized data"
  end

  def self.__own_members__
    @__members__
  end

  # Every member, and nothing else. Keys may be Strings.
  def initialize(**keywords)
    members = self.class.__members__
    values = {}
    unknown = []
    keywords.each do |key, value|
      name = Data.__key__(key)
      if members.include?(name)
        values[name] = value
      else
        unknown.push(key.is_a?(Symbol) ? key : name.to_s)
      end
    end
    missing = members.reject { |member| values.key?(member) }
    unless missing.empty?
      raise ArgumentError, "missing keyword" + (missing.size > 1 ? "s" : "") + ": " +
                           missing.map { |name| name.inspect }.join(", ")
    end
    Data.__unknown__(unknown)
    ordered = {}
    members.each { |member| ordered[member] = values[member] }
    @__values__ = ordered
    freeze
  end

  # A member's name as a Symbol, from a Symbol, a String, or what converts
  # with `to_str`.
  def self.__key__(key)
    return key if key.is_a?(Symbol)
    return key.to_sym if key.is_a?(String)
    unless key.respond_to?(:to_str)
      raise TypeError, key.inspect + " is not a symbol nor a string"
    end
    text = key.to_str
    unless text.is_a?(String)
      raise TypeError, "can't convert " + key.class.to_s + " to String (" +
                       key.class.to_s + "#to_str gives " + text.class.to_s + ")"
    end
    text.to_sym
  end

  def self.__unknown__(unknown)
    return if unknown.empty?
    raise ArgumentError, "unknown keyword" + (unknown.size > 1 ? "s" : "") + ": " +
                         unknown.map { |name| name.inspect }.join(", ")
  end

  # nil for every member on an object that was allocated and never
  # initialized, which is what `inspect` on one shows.
  def __data_values__
    return @__values__ unless @__values__.nil?
    blank = {}
    self.class.__members__.each { |member| blank[member] = nil }
    blank
  end

  def members
    self.class.__members__.dup
  end

  def to_h
    return __data_values__.dup unless block_given?
    out = {}
    __data_values__.each do |member, value|
      pair = yield(member, value)
      unless pair.is_a?(Array) || pair.respond_to?(:to_ary)
        raise TypeError, "wrong element type " + pair.class.to_s + " (expected array)"
      end
      pair = pair.to_ary unless pair.is_a?(Array)
      unless pair.size == 2
        raise ArgumentError, "element has wrong array length (expected 2, was " + pair.size.to_s + ")"
      end
      out[pair[0]] = pair[1]
    end
    out
  end

  def deconstruct
    __data_values__.values
  end

  # A copy with some members replaced. No arguments is the object itself.
  def with(*args, **keywords)
    unless args.empty?
      raise ArgumentError, "wrong number of arguments (given " + args.size.to_s + ", expected 0)"
    end
    return self if keywords.empty?
    # Through `initialize`, so a class that has its own sees the new values.
    merged = __data_values__.dup
    keywords.each { |key, value| merged[key.is_a?(String) ? key.to_sym : key] = value }
    copy = self.class.allocate
    copy.__send__(:initialize, **merged)
    copy
  end

  def initialize_copy(other)
    @__values__ = other.__data_values__.dup
    freeze
  end

  def deconstruct_keys(keys)
    return to_h if keys.nil?
    unless keys.is_a?(Array)
      raise TypeError, "wrong argument type " + keys.class.to_s + " (expected Array or nil)"
    end
    values = __data_values__
    out = {}
    return out if keys.size > values.size
    keys.each do |key|
      name = Data.__key__(key)
      return out unless values.key?(name)
      # Keyed as asked, except that what was converted is keyed by the String.
      out[key.is_a?(Symbol) || key.is_a?(String) ? key : name.to_s] = values[name]
    end
    out
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Data) && other.class.equal?(self.class)
    Struct.__guard__(:==, self, other, true) { __data_values__ == other.__data_values__ }
  end

  def eql?(other)
    return true if equal?(other)
    return false unless other.is_a?(Data) && other.class.equal?(self.class)
    Struct.__guard__(:eql?, self, other, true) { __data_values__.eql?(other.__data_values__) }
  end

  def hash
    Struct.__guard__(:hash, self, nil, self.class.hash) { [self.class, __data_values__].hash }
  end

  def inspect
    name = Struct.__real_name__(self.class)
    name = nil if !name.nil? && name.start_with?("#<")
    head = name.nil? ? "#<data " : "#<data " + name + " "
    # An anonymous class is named after all where the walk meets itself.
    again = "#<data " + (name.nil? ? self.class.inspect : name) + ":...>"
    Struct.__guard__(:inspect, self, nil, again) do
      parts = __data_values__.map { |member, value| member.to_s + "=" + value.inspect }
      # A named one with no members has no space to close; an anonymous
      # one keeps it: `#<data >`. Measured.
      (name.nil? ? head + parts.join(", ") : (head + parts.join(", ")).rstrip) + ">"
    end
  end
  alias to_s inspect
end
