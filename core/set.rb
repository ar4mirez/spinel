# Set — a collection of distinct objects, a core class since Ruby 4.0 (#48).
#
# Ruby over a Hash: an element is a key, so "distinct" means what it means
# for Hash keys — `hash` then `eql?` — and insertion order is kept. Every
# answer and message here is measured on ruby 4.0.6.
class Set
  include Enumerable

  def self.[](*elements)
    new(elements)
  end

  def initialize(enum = nil, &block)
    @__hash__ = {}
    return if enum.nil?
    if block
      __each_of__(enum) { |element| add(block.call(element)) }
    else
      merge(enum)
    end
  end

  def initialize_copy(other)
    @__hash__ = other.__hash_table__.dup
    self
  end

  def freeze
    @__hash__.freeze
    super
  end

  def __hash_table__ = @__hash__

  # Each element of `enum`, which must be something that can be walked: a
  # Set, or anything with `each_entry`. A lone Integer is not.
  def __each_of__(enum, &block)
    if enum.is_a?(Set)
      enum.each(&block)
    elsif enum.respond_to?(:each_entry)
      enum.each_entry(&block)
    else
      raise ArgumentError, "value must be enumerable"
    end
  end

  def __set__(other)
    raise ArgumentError, "value must be a set" unless other.is_a?(Set)
    other
  end

  def compare_by_identity
    Kernel.__check_frozen__(self)
    @__hash__.compare_by_identity
    self
  end

  def compare_by_identity?
    @__hash__.compare_by_identity?
  end

  def size
    @__hash__.size
  end
  alias length size

  def empty?
    @__hash__.empty?
  end

  def include?(element)
    @__hash__.key?(element)
  end
  alias member? include?
  alias === include?

  def each(&block)
    return enum_for(:each) { size } unless block
    @__hash__.each_key(&block)
    self
  end

  def to_a
    @__hash__.keys
  end

  def to_set(*given, &block)
    if given.empty?
      return self if instance_of?(Set) && block.nil?
      return Set.new(self, &block)
    end
    __warning__("passing arguments to Enumerable#to_set is deprecated", false, :deprecated)
    given[0].new(self, *given.__take__(1, given.size - 1), &block)
  end

  def add(element)
    Kernel.__check_frozen__(self)
    @__hash__[element] = true
    self
  end
  alias << add

  def add?(element)
    return nil if include?(element)
    add(element)
  end

  def delete(element)
    Kernel.__check_frozen__(self)
    @__hash__.delete(element)
    self
  end

  def delete?(element)
    return nil unless include?(element)
    delete(element)
  end

  def clear
    Kernel.__check_frozen__(self)
    @__hash__.clear
    self
  end

  def replace(enum)
    Kernel.__check_frozen__(self)
    if enum.is_a?(Set)
      @__hash__ = enum.__hash_table__.dup
      return self
    end
    # Checked before anything is thrown away.
    elements = []
    __each_of__(enum) { |element| elements.push(element) }
    @__hash__.clear
    elements.each { |element| @__hash__[element] = true }
    self
  end

  def merge(*enums)
    Kernel.__check_frozen__(self)
    enums.each do |enum|
      __each_of__(enum) { |element| @__hash__[element] = true }
    end
    self
  end

  def subtract(enum)
    Kernel.__check_frozen__(self)
    __each_of__(enum) { |element| @__hash__.delete(element) }
    self
  end

  def reset
    self
  end

  def delete_if
    return enum_for(:delete_if) { size } unless block_given?
    Kernel.__check_frozen__(self)
    to_a.each { |element| @__hash__.delete(element) if yield(element) }
    self
  end

  def keep_if
    return enum_for(:keep_if) { size } unless block_given?
    Kernel.__check_frozen__(self)
    to_a.each { |element| @__hash__.delete(element) unless yield(element) }
    self
  end

  # The bang forms answer nil when nothing changed.
  def reject!(&block)
    return enum_for(:reject!) { size } unless block
    before = size
    delete_if(&block)
    size == before ? nil : self
  end

  def select!(&block)
    return enum_for(:select!) { size } unless block
    before = size
    keep_if(&block)
    size == before ? nil : self
  end
  alias filter! select!

  def collect!
    return enum_for(:collect!) { size } unless block_given?
    Kernel.__check_frozen__(self)
    mapped = to_a.map { |element| yield(element) }
    # A fresh table: what comes out of the block is compared the ordinary
    # way, whatever the set did before. Measured.
    @__hash__ = {}
    mapped.each { |element| @__hash__[element] = true }
    self
  end
  alias map! collect!

  def |(enum)
    dup.merge(enum)
  end
  alias + |
  alias union |

  def -(enum)
    dup.subtract(enum)
  end
  alias difference -

  # A new set, compared the ordinary way even when this one is not. Measured:
  # `^` keeps the identity flag and `&` does not.
  def &(enum)
    out = self.class.new
    __each_of__(enum) { |element| out.add(element) if include?(element) }
    out
  end
  alias intersection &

  # In one or the other, and not both.
  def ^(enum)
    other = enum.is_a?(Set) ? enum : self.class.new(enum)
    out = self.class.new
    out.compare_by_identity if compare_by_identity?
    each { |element| out.add(element) unless other.include?(element) }
    other.each { |element| out.add(element) unless include?(element) }
    out
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Set) && size == other.size
    # A set that tells elements apart by identity is a different kind of set.
    return false unless compare_by_identity? == other.compare_by_identity?
    other.all? { |element| include?(element) }
  end
  alias eql? ==

  # The same whatever order the elements went in.
  def hash
    # A set that contains itself would ask itself for its hash for ever;
    # where it recurs it counts for nothing, as CRuby's recursion guard has it.
    open = Set.__hashing__
    return 0 if open.any? { |outer| outer.equal?(self) }
    open.push(self)
    begin
      total = size
      @__hash__.each_key { |element| total = total ^ element.hash }
      total
    ensure
      open.pop
    end
  end

  def self.__hashing__
    @__hashing__ ||= []
  end

  def subset?(other)
    __set__(other)
    size <= other.size && all? { |element| other.include?(element) }
  end
  alias <= subset?

  def superset?(other)
    __set__(other)
    size >= other.size && other.all? { |element| include?(element) }
  end
  alias >= superset?

  def proper_subset?(other)
    __set__(other)
    size < other.size && all? { |element| other.include?(element) }
  end
  alias < proper_subset?

  def proper_superset?(other)
    __set__(other)
    size > other.size && other.all? { |element| include?(element) }
  end
  alias > proper_superset?

  # -1, 0 or 1 when one contains the other, and nil when neither does.
  def <=>(other)
    return nil unless other.is_a?(Set)
    order = size <=> other.size
    if order < 0
      proper_subset?(other) ? -1 : nil
    elsif order > 0
      proper_superset?(other) ? 1 : nil
    else
      self == other ? 0 : nil
    end
  end

  def intersect?(enum)
    found = false
    __each_of__(enum) { |element| found = true if include?(element) }
    found
  end

  def disjoint?(enum)
    !intersect?(enum)
  end

  def classify
    return enum_for(:classify) { size } unless block_given?
    groups = {}
    each do |element|
      key = yield(element)
      groups[key] = self.class.new unless groups.key?(key)
      groups[key].add(element)
    end
    groups
  end

  # With a one-argument block, the sets `classify` makes. With two, the
  # groups of elements the block connects, directly or through others.
  def divide(&block)
    return enum_for(:divide) { size } unless block
    if block.arity == 2
      elements = to_a
      group_of = {}.compare_by_identity
      out = self.class.new
      elements.each do |start|
        next if group_of.key?(start)
        group = self.class.new
        pending = [start]
        group_of[start] = group
        until pending.empty?
          current = pending.pop
          group.add(current)
          elements.each do |other|
            next if group_of.key?(other)
            if block.call(current, other) && block.call(other, current)
              group_of[other] = group
              pending.push(other)
            end
          end
        end
        out.add(group)
      end
      out
    else
      Set.new(classify(&block).values)
    end
  end

  def flatten
    self.class.new.__flatten_merge__(self, [])
  end

  def flatten!
    return nil unless any? { |element| element.is_a?(Set) }
    replace(flatten)
  end

  def __flatten_merge__(set, seen)
    set.each do |element|
      if element.is_a?(Set)
        if seen.any? { |outer| outer.equal?(element) }
          raise ArgumentError, "tried to flatten recursive Set"
        end
        seen.push(element)
        __flatten_merge__(element, seen)
        seen.pop
      else
        add(element)
      end
    end
    self
  end

  def join(separator = nil)
    to_a.join(separator)
  end

  # `Set[1, 2]` for a Set, and `#<Name: {1, 2}>` for a subclass. A set that
  # contains itself prints `Set[...]` where it recurs.
  def inspect
    open = Set.__inspecting__
    if open.any? { |outer| outer.equal?(self) }
      return instance_of?(Set) ? "Set[...]" : "#<#{self.class}: {...}>"
    end
    open.push(self)
    begin
      body = to_a.map { |element| element.inspect }.join(", ")
    ensure
      open.pop
    end
    instance_of?(Set) ? "Set[#{body}]" : "#<#{self.class}: {#{body}}>"
  end
  alias to_s inspect

  def self.__inspecting__
    @__inspecting__ ||= []
  end
end

module Enumerable
  # `*given` rather than a default so that being handed a class at all can
  # be told from not being handed one: Ruby 4.0 deprecates the argument.
  def to_set(*given, &block)
    return Set.new(self, &block) if given.empty?
    __warning__("passing arguments to Enumerable#to_set is deprecated", false, :deprecated)
    given[0].new(self, *given.__take__(1, given.size - 1), &block)
  end
end
