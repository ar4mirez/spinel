# Struct — a class made of named members (#30).
#
# `Struct.new(:a, :b)` answers a new class whose instances hold one value per
# member, with an accessor pair each. It is written in Ruby: a struct's values
# are an Array in a hidden instance variable, and its members are an Array on
# the class. Nothing here needed a primitive.
#
# `Struct.new` does two jobs in Ruby, and which one depends on the receiver.
# On `Struct`, or on a subclass that has no members yet, it *defines* a struct
# class. On a class that has members it makes an instance. The second is the
# `new` each defined class is given as a singleton method, so the first is all
# `Struct.new` itself has to be.
class Struct
  include Enumerable

  def self.new(*given, keyword_init: nil, &block)
    name = nil
    if !given.empty? && !given[0].is_a?(Symbol)
      first = given[0]
      if first.nil?
        given.shift
      elsif first.is_a?(String) || first.respond_to?(:to_str)
        name = first.is_a?(String) ? first : first.to_str
        given.shift
      end
    end
    members = given.map do |member|
      unless member.is_a?(Symbol) || member.is_a?(String)
        raise TypeError, member.inspect + " is not a symbol nor a string"
      end
      member.to_sym
    end
    members.each_with_index do |member, index|
      if members.index(member) != index
        raise ArgumentError, "duplicate member: " + member.to_s
      end
    end

    struct = Class.new(self)
    struct.__define_struct__(members, keyword_init)
    unless name.nil?
      unless name =~ /\A[A-Z]\w*\z/
        raise NameError.new("identifier " + name + " needs to be constant", name.to_sym, receiver: self)
      end
      # Under the receiver: `Struct::Name` for `Struct.new`, and a subclass's
      # own namespace for a subclass.
      if const_defined?(name, false)
        __send__(:remove_const, name)
        __send__(:__warning__, "redefining constant " + self.name.to_s + "::" + name)
      end
      const_set(name, struct)
    end
    struct.class_eval(&block) unless block.nil?
    struct
  end

  # What turns a fresh class into a struct class: its members, its accessors,
  # and the `new` that makes instances rather than more classes.
  def self.__define_struct__(members, keyword_init)
    @__members__ = members
    @__keyword_init__ = keyword_init
    members.each_with_index do |member, index|
      define_method(member) { __struct_values__[index] }
      define_method((member.to_s + "=").to_sym) do |value|
        __struct_check_frozen__
        __struct_values__[index] = value
      end
    end
    class << self
      def new(*args, **keywords, &block)
        instance = allocate
        instance.__send__(:initialize, *args, **keywords, &block)
        instance
      end
      alias [] new

      def members
        __members__.dup
      end

      def keyword_init?
        mode = __keyword_init__
        mode.nil? ? nil : (mode ? true : false)
      end

      def inspect
        text = super
        __keyword_init__ ? text + "(keyword_init: true)" : text
      end
    end
  end

  # The members, from whichever ancestor was defined as a struct: a class that
  # inherits from `Struct.new(:a)` has the same ones.
  def self.__members__
    scope = self
    while scope.respond_to?(:__own_members__)
      own = scope.__own_members__
      return own unless own.nil?
      scope = scope.superclass
    end
    raise TypeError, "uninitialized struct"
  end

  def self.__own_members__
    @__members__
  end

  def self.__keyword_init__
    scope = self
    while scope.respond_to?(:__own_members__)
      return scope.__own_keyword_init__ unless scope.__own_members__.nil?
      scope = scope.superclass
    end
    nil
  end

  def self.__own_keyword_init__
    @__keyword_init__
  end

  # Positional values fill the members in order and the rest are nil. Keywords
  # name them instead — always under `keyword_init: true`, and under the
  # default when keywords are all that was given.
  def initialize(*args, **keywords)
    members = self.class.__members__
    mode = self.class.__keyword_init__
    # Every member is set, to nil where nothing was given — which overwrites
    # what a subclass's `initialize` assigned before calling `super`.
    @__values__ = Array.new(members.size)
    if mode
      # One positional Hash is the keywords, written the old way.
      if keywords.empty? && args.size == 1 && args[0].is_a?(Hash)
        keywords = args[0]
        args = []
      end
      unless args.empty?
        raise ArgumentError, "wrong number of arguments (given " + args.size.to_s + ", expected 0)"
      end
      __struct_fill_keywords__(members, keywords)
    elsif mode.nil? && args.empty? && !keywords.empty?
      __struct_fill_keywords__(members, keywords)
    else
      args = args + [keywords] unless keywords.empty?
      raise ArgumentError, "struct size differs" if args.size > members.size
      args.each_with_index { |value, index| __struct_values__[index] = value }
    end
    nil
  end

  def __struct_fill_keywords__(members, keywords)
    unknown = keywords.keys.reject { |key| members.include?(key) }
    unless unknown.empty?
      raise ArgumentError, "unknown keywords: " + unknown.map { |key| key.to_s }.join(", ")
    end
    keywords.each { |key, value| __struct_values__[members.index(key)] = value }
  end

  def __struct_check_frozen__
    if frozen?
      raise FrozenError.new("can't modify frozen " + self.class.to_s + ": " + inspect, receiver: self)
    end
  end

  def initialize_copy(other)
    @__values__ = other.__struct_values__.dup
    self
  end

  # The values, made on first use: a subclass may assign a member before its
  # `initialize` reaches this class's.
  def __struct_values__
    @__values__ ||= Array.new(self.class.__members__.size)
  end

  def members
    self.class.__members__.dup
  end

  def to_a
    __struct_values__.dup
  end
  alias deconstruct to_a
  alias values to_a

  def size
    __struct_values__.size
  end
  alias length size

  # A member is named by Symbol or String, or counted from either end by an
  # Integer or what converts to one.
  def __struct_index__(key)
    members = self.class.__members__
    if key.is_a?(Symbol) || key.is_a?(String)
      index = members.index(key.to_sym)
      if index.nil?
        raise NameError.new("no member '" + key.to_s + "' in struct", key.to_sym, receiver: self)
      end
      return index
    end
    index = Integer.__index__(key)
    size = members.size
    if index >= size
      raise IndexError, "offset " + index.to_s + " too large for struct(size:" + size.to_s + ")"
    end
    if index < -size
      raise IndexError, "offset " + index.to_s + " too small for struct(size:" + size.to_s + ")"
    end
    index < 0 ? index + size : index
  end

  def [](key)
    __struct_values__[__struct_index__(key)]
  end

  def []=(key, value)
    index = __struct_index__(key)
    __struct_check_frozen__
    __struct_values__[index] = value
  end

  def each(&block)
    return to_enum(:each) { size } if block.nil?
    __struct_values__.each(&block)
    self
  end

  def each_pair
    return to_enum(:each_pair) { size } unless block_given?
    members = self.class.__members__
    members.each_with_index { |member, index| yield [member, __struct_values__[index]] }
    self
  end

  def to_h
    out = {}
    members = self.class.__members__
    members.each_with_index do |member, index|
      if block_given?
        pair = yield(member, __struct_values__[index])
        unless pair.is_a?(Array) || pair.respond_to?(:to_ary)
          raise TypeError, "wrong element type " + pair.class.to_s + " (expected array)"
        end
        pair = pair.to_ary unless pair.is_a?(Array)
        unless pair.size == 2
          raise ArgumentError, "element has wrong array length (expected 2, was " + pair.size.to_s + ")"
        end
        out[pair[0]] = pair[1]
      else
        out[member] = __struct_values__[index]
      end
    end
    out
  end

  # For `in {a:, b:}`: the named members that exist, stopping at the first
  # that does not. nil asks for all of them.
  def deconstruct_keys(keys)
    return to_h if keys.nil?
    unless keys.is_a?(Array)
      raise TypeError, "wrong argument type " + keys.class.to_s + " (expected Array or nil)"
    end
    out = {}
    return out if keys.size > size
    members = self.class.__members__
    keys.each do |key|
      index = if key.is_a?(Symbol) || key.is_a?(String)
        members.index(key.to_sym)
      else
        at = Integer.__index__(key)
        at >= -size && at < size ? at : nil
      end
      return out if index.nil?
      out[key] = __struct_values__[index]
    end
    out
  end

  # An Integer past either end is an error here, where an Array answers nil.
  def values_at(*selectors)
    selectors.each { |selector| __struct_index__(selector) if selector.is_a?(Integer) }
    __struct_values__.values_at(*selectors)
  end

  def select(&block)
    return to_enum(:select) { size } if block.nil?
    __struct_values__.select(&block)
  end
  alias filter select

  def dig(key, *rest)
    value = begin
      self[key]
    rescue IndexError, NameError
      nil
    end
    return value if rest.empty? || value.nil?
    unless value.respond_to?(:dig)
      raise TypeError, value.class.to_s + " does not have #dig method"
    end
    value.dig(*rest)
  end

  # Two structs are equal when they are of one class and their values are.
  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Struct) && other.class.equal?(self.class)
    Struct.__guard__(:==, self, other, true) { @__values__ == other.__struct_values__ }
  end

  def eql?(other)
    return true if equal?(other)
    return false unless other.is_a?(Struct) && other.class.equal?(self.class)
    Struct.__guard__(:eql?, self, other, true) { __struct_values__.eql?(other.__struct_values__) }
  end

  # A struct that contains itself hashes as its class alone, and so does
  # whatever struct the walk started from: CRuby's `rb_exec_recursive_outer`.
  # That is what gives two `eql?` recursive structs one hash.
  def hash
    open = Struct.__hashing__
    throw :__struct_hash_recursion__ if open.any? { |seen| seen.equal?(self) }
    outermost = open.empty?
    open.push(self)
    begin
      return [self.class, __struct_values__].hash unless outermost
      answer = catch(:__struct_hash_recursion__) { [self.class, __struct_values__].hash }
      answer.nil? ? self.class.hash : answer
    ensure
      open.pop
    end
  end

  def self.__hashing__
    @__hashing__ ||= []
  end

  def inspect
    # The class's real name, whatever a `name` method of its own says, and no
    # name at all for a class nested in an anonymous one.
    name = Struct.__real_name__(self.class)
    name = nil if !name.nil? && name.start_with?("#<")
    head = name.nil? ? "#<struct " : "#<struct " + name + " "
    Struct.__guard__(:inspect, self, nil, head.rstrip + ":...>") do
      members = self.class.__members__
      parts = []
      members.each_with_index do |member, index|
        parts.push(member.to_s + "=" + __struct_values__[index].inspect)
      end
      # A named one with no members has no space to close; an anonymous
      # one keeps it: `#<data >`. Measured.
      (name.nil? ? head + parts.join(", ") : (head + parts.join(", ")).rstrip) + ">"
    end
  end
  alias to_s inspect

  def self.__real_name__(struct)
    Module.instance_method(:name).bind(struct).call
  end

  # A struct that holds itself must not recurse forever: the walk that meets
  # the same struct again answers `again` instead.
  def self.__guard__(kind, a, b, again)
    @__open__ ||= []
    if @__open__.any? { |k, x, y| k == kind && x.equal?(a) && y.equal?(b) }
      return again
    end
    @__open__.push([kind, a, b])
    begin
      yield
    ensure
      @__open__.pop
    end
  end
end
