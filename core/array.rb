# Array.
#
# `[]`, `[]=`, `size`, `length`, `push`, `append`, `pop` and `+` are primitives:
# they read or write a raw slot run, and grow it. Everything below is Ruby.
#
# The representation is two slots — storage and length — so that `<<` mutates
# the array the caller is holding rather than allocating a new one. See
# `interp.rs`, "Array".
class Array
  # `Array.new`, `Array.new(3)`, `Array.new(3, :x)`, `Array.new(3) { |i| ... }`,
  # and `Array.new([1, 2])`, which copies. Reachable as `initialize` too, where
  # the receiver may already hold elements — hence the `clear`: Ruby *replaces*
  # the contents rather than appending to them.
  def initialize(*given)
    if given.size > 2
      raise ArgumentError, "wrong number of arguments (given " + given.size.to_s + ", expected 0..2)"
    end
    size = given.size > 0 ? given[0] : 0
    default = given.size > 1 ? given[1] : nil
    if size.is_a?(Array)
      # `Array.new([1, 2], :x)` is a TypeError, not an arity error: the first
      # argument was taken as a size and an Array is not one.
      unless given.size < 2
        raise TypeError, "no implicit conversion of Array into Integer"
      end
      clear
      size.each { |element| push(element) }
      return self
    end
    unless size.is_a?(Integer)
      raise TypeError, "no implicit conversion into Integer"
    end
    raise ArgumentError, "negative array size" if size < 0
    clear
    i = 0
    while i < size
      push(block_given? ? yield(i) : default)
      i = i + 1
    end
    self
  end

  def <<(element)
    push(element)
  end

  def empty?
    size == 0
  end

  # `first` and `last` answer one element; `first(n)` and `last(n)` answer a new
  # array of n. Which one is picked by whether an argument was *given*, not by
  # whether it was nil: `first(nil)` is a TypeError, not `first`.
  def first(*count)
    return self[0] if count.size == 0
    __take__(0, __count__(count[0]))
  end

  def last(*count)
    return self[size - 1] if count.size == 0
    wanted = __count__(count[0])
    from = size - wanted
    from = 0 if from < 0
    __take__(from, wanted)
  end

  # A count argument is converted with `to_int`, and something that has no
  # `to_int` is a TypeError rather than an ArgumentError about arity.
  def __count__(count)
    unless count.is_a?(Integer)
      if count.nil? || !count.respond_to?(:to_int)
        raise TypeError, "no implicit conversion into Integer"
      end
      count = count.to_int
      unless count.is_a?(Integer)
        raise TypeError, "can't convert to Integer"
      end
    end
    raise ArgumentError, "negative array size" if count < 0
    count
  end

  def __take__(from, wanted)
    out = []
    i = from
    last = from + wanted
    while i < size && i < last
      out.push(self[i])
      i = i + 1
    end
    out
  end

  def each
    i = 0
    while i < size
      yield self[i]
      i = i + 1
    end
    self
  end

  def each_with_index
    i = 0
    while i < size
      yield self[i], i
      i = i + 1
    end
    self
  end

  def each_index
    i = 0
    while i < size
      yield i
      i = i + 1
    end
    self
  end

  def map
    out = []
    each { |element| out.push(yield(element)) }
    out
  end

  def collect
    out = []
    each { |element| out.push(yield(element)) }
    out
  end

  def select
    out = []
    each { |element| out.push(element) if yield(element) }
    out
  end

  def filter
    out = []
    each { |element| out.push(element) if yield(element) }
    out
  end

  def reject
    out = []
    each { |element| out.push(element) unless yield(element) }
    out
  end

  # `find(ifnone)` calls `ifnone` when nothing matched. Ruby does not check that
  # it is callable up front, so a non-callable raises NoMethodError at the point
  # it would have been called — which is what `find_spec.rb` asserts.
  def find(ifnone = nil)
    each { |element| return element if yield(element) }
    return nil if ifnone.nil?
    ifnone.call
  end

  def detect(ifnone = nil)
    each { |element| return element if yield(element) }
    return nil if ifnone.nil?
    ifnone.call
  end

  def include?(wanted)
    each { |element| return true if element == wanted }
    false
  end

  def index(wanted)
    i = 0
    while i < size
      return i if self[i] == wanted
      i = i + 1
    end
    nil
  end

  def any?
    each { |element| return true if block_given? ? yield(element) : element }
    false
  end

  def all?
    each { |element| return false unless block_given? ? yield(element) : element }
    true
  end

  def none?
    !any? { |element| block_given? ? yield(element) : element }
  end

  def count
    return size unless block_given?
    n = 0
    each { |element| n = n + 1 if yield(element) }
    n
  end

  def sum(initial = 0)
    total = initial
    each { |element| total = total + (block_given? ? yield(element) : element) }
    total
  end

  # With a block, the block *is* the comparison — `min { |a, b| ... }` — and it
  # is called with the candidate first and the incumbent second.
  def min
    return nil if empty?
    best = self[0]
    i = 1
    while i < size
      element = self[i]
      order = block_given? ? yield(element, best) : (element <=> best)
      best = element if order < 0
      i = i + 1
    end
    best
  end

  def max
    return nil if empty?
    best = self[0]
    i = 1
    while i < size
      element = self[i]
      order = block_given? ? yield(element, best) : (element <=> best)
      best = element if order > 0
      i = i + 1
    end
    best
  end

  def reverse
    out = []
    i = size - 1
    while i >= 0
      out.push(self[i])
      i = i - 1
    end
    out
  end

  def reverse_each
    i = size - 1
    while i >= 0
      yield self[i]
      i = i - 1
    end
    self
  end

  # A shallow copy has to be built rather than copied: `Kernel#dup` copies the
  # cell, and an Array's cell holds a *pointer* to its storage, so the copy
  # would share it and `b << 1` would show up in `a`.
  def dup
    out = []
    each { |element| out.push(element) }
    out
  end

  def to_a
    self
  end

  def to_ary
    self
  end

  # `shift` answers the first element; `shift(n)` answers a new array of the
  # first n. Both leave the rest behind, in this same array.
  def shift(*count)
    if count.size > 1
      raise ArgumentError, "wrong number of arguments (given " + count.size.to_s + ", expected 0..1)"
    end
    given = count.size > 0
    wanted = given ? __count__(count[0]) : 1
    taken = __take__(0, wanted)
    rest = __take__(wanted, size)
    clear
    rest.each { |element| push(element) }
    return taken if given
    taken.empty? ? nil : taken[0]
  end

  def unshift(*elements)
    rest = dup
    clear
    elements.each { |element| push(element) }
    rest.each { |element| push(element) }
    self
  end

  def clear
    pop until empty?
    self
  end

  def concat(other)
    other.each { |element| push(element) }
    self
  end

  def join(separator = "")
    out = ""
    i = 0
    while i < size
      out = out + self[i].to_s
      out = out + separator if i < size - 1
      i = i + 1
    end
    out
  end

  def inspect
    out = "["
    i = 0
    while i < size
      out = out + self[i].inspect
      out = out + ", " if i < size - 1
      i = i + 1
    end
    out + "]"
  end

  def to_s
    inspect
  end
end
