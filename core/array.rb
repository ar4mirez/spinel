# Array.
#
# `[]`, `[]=`, `size`, `length`, `push`, `append`, `pop` and `+` are primitives:
# they read or write a raw slot run, and grow it. Everything below is Ruby.
#
# The representation is two slots — storage and length — so that `<<` mutates
# the array the caller is holding rather than allocating a new one. See
# `interp.rs`, "Array".
class Array
  include Enumerable

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

  # The right-hand side of a multiple assignment, as an Array (#154).
  #
  # Ruby converts with `to_ary`, not `to_a`: an object that defines only `to_a`
  # is *not* spread, it becomes the single value. Anything without `to_ary` is
  # wrapped, which is why `a, b = nil` leaves both nil rather than raising.
  def self.__masgn_array__(value)
    return value if value.is_a?(Array)
    return value.to_ary if value.respond_to?(:to_ary)
    [value]
  end

  # Cut this array into one value per assignment target (#154).
  #
  # `befores` targets before the splat, `rest` whether there is one, `afters`
  # targets after it. The answer always has `befores + (rest ? 1 : 0) + afters`
  # entries, padded with nil, so the compiler can index it by target position
  # without knowing how long the right-hand side turned out to be.
  def __masgn_spread__(befores, rest, afters)
    out = []
    i = 0
    while i < befores
      out.push(self[i])
      i = i + 1
    end
    # Where the targets after the splat start. When the right-hand side is
    # shorter than the target list they fill from here rather than from the
    # end, which is why `*a, b, c = [1]` leaves b as 1 and c as nil.
    stop = size - afters
    stop = befores if stop < befores
    if rest
      middle = []
      j = befores
      while j < stop
        middle.push(self[j])
        j = j + 1
      end
      out.push(middle)
    end
    k = 0
    while k < afters
      out.push(self[stop + k])
      k = k + 1
    end
    out
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
    # The size block is what makes `[1, 2].each.size` answer 2 rather than nil,
    # which is what `Enumerator::Chain#size` sums.
    return to_enum(:each) { size } unless block_given?
    i = 0
    while i < size
      yield self[i]
      i = i + 1
    end
    self
  end

  def each_with_index
    return to_enum(:each_with_index) unless block_given?
    i = 0
    while i < size
      yield self[i], i
      i = i + 1
    end
    self
  end

  def each_index
    return to_enum(:each_index) unless block_given?
    i = 0
    while i < size
      yield i
      i = i + 1
    end
    self
  end

  def map
    return to_enum(:map) unless block_given?
    out = []
    each { |element| out.push(yield(element)) }
    out
  end

  def collect
    return to_enum(:collect) unless block_given?
    out = []
    each { |element| out.push(yield(element)) }
    out
  end

  def select
    return to_enum(:select) unless block_given?
    out = []
    each { |element| out.push(element) if yield(element) }
    out
  end

  def filter
    return to_enum(:filter) unless block_given?
    out = []
    each { |element| out.push(element) if yield(element) }
    out
  end

  def reject
    return to_enum(:reject) unless block_given?
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




  # The bare count is the length, which Array knows without walking. Anything
  # else — an item to match, or a block — is Enumerable's.
  def count(*item)
    return size if item.empty? && !block_given?
    super
  end

  # Kahan-Babuska compensated summation once a Float enters the sum, which is
  # what Ruby does and what makes `[2.78, 5.0, 2.5, ...].sum` answer 50.0 where
  # a left fold answers 50.00000000000001. Integers stay exact and never enter
  # the compensated path, so nothing is rounded that need not be.
  def sum(initial = 0)
    total = initial
    compensation = 0.0
    floating = total.is_a?(Float)
    each do |element|
      value = block_given? ? yield(element) : element
      unless floating
        unless value.is_a?(Float)
          total = total + value
          next
        end
        # First Float: carry the exact integer total over and start compensating.
        floating = true
        total = total + value
        next
      end
      running = total + value
      if total.abs >= value.abs
        compensation = compensation + ((total - running) + value)
      else
        compensation = compensation + ((value - running) + total)
      end
      total = running
    end
    floating ? total + compensation : total
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


  # A shallow copy has to be built rather than copied: `Kernel#dup` copies the
  # cell, and an Array's cell holds a *pointer* to its storage, so the copy
  # would share it and `b << 1` would show up in `a`.
  def dup
    out = []
    each { |element| out.push(element) }
    out
  end

  # ponytail: the frozen state and not the singleton class, as `Hash#clone`
  # does and for its reason; both go with #201's `Kernel#clone`.
  def clone(freeze: nil)
    __needs_kernel_clone__ unless singleton_methods.empty?
    copy = dup
    copy.freeze if freeze.nil? ? frozen? : freeze
    copy
  end

  # Element by element, then by length: the first pair that disagrees decides,
  # and two arrays that agree as far as the shorter one goes are ordered by size.
  # `nil` when the operand is not an array, or when any pair is not comparable —
  # `[1] <=> [:a]` is nil, not an error, because `<=>` reports "no opinion"
  # rather than raising. Measured on ruby 4.0.6.
  def <=>(other)
    return nil unless other.is_a?(Array)
    return 0 if equal?(other)
    shorter = size < other.size ? size : other.size
    i = 0
    while i < shorter
      cmp = (self[i] <=> other[i])
      return nil if cmp.nil?
      return cmp unless cmp == 0
      i = i + 1
    end
    size <=> other.size
  end

  # Element by element with each element's own `eql?`, so `[1].eql?([1.0])` is
  # false where `==` is true. Same recursion rule as `==`.
  def eql?(other)
    return true if equal?(other)
    return false unless other.is_a?(Array) && size == other.size
    comparing = Array.__comparing__
    return true if comparing.any? { |a, b| a.equal?(self) && b.equal?(other) }
    comparing.push([self, other])
    begin
      i = 0
      while i < size
        return false unless self[i].eql?(other[i])
        i = i + 1
      end
    ensure
      comparing.pop
    end
    true
  end

  # A fold of the elements' own `hash` (#22), so an Array holding a Hash, or a
  # key class with a custom `hash`, digests by content the way `eql?` compares.
  # The class is not in it: measured, a subclass hashes like a plain Array.
  def hash
    __recursive_hash__(__hash_combine__(:__spinel_recursive_array__, size)) do
      digest = __hash_combine__(:__spinel_array__, size)
      i = 0
      while i < size
        digest = __hash_combine__(digest, __element_hash__(self[i]))
        i = i + 1
      end
      digest
    end
  end

  # Element by element, each compared with its own `==` — which is what lets an
  # Array of Hashes compare, now that `Hash#==` is Ruby (#22). Measured on ruby
  # 4.0.7: a non-Array that has `to_ary` is asked `other == self`, and two
  # Arrays that contain themselves compare equal rather than recursing forever.
  def ==(other)
    return true if equal?(other)
    unless other.is_a?(Array)
      return false unless other.respond_to?(:to_ary)
      return other == self ? true : false
    end
    return false unless size == other.size
    comparing = Array.__comparing__
    return true if comparing.any? { |a, b| a.equal?(self) && b.equal?(other) }
    comparing.push([self, other])
    begin
      i = 0
      while i < size
        return false unless self[i] == other[i]
        i = i + 1
      end
    ensure
      comparing.pop
    end
    true
  end

  # The pairs `==` is part-way through, innermost last. Per heap, on `Array`.
  def self.__comparing__
    @__comparing__ ||= []
  end

  # A subclass's instance answers a plain Array copy. Measured.
  def to_a
    instance_of?(Array) ? self : Array.new(self)
  end

  # An Array is its own array pattern subject (#165).
  def deconstruct
    self
  end

  def to_ary
    self
  end

  # `shift` answers the first element; `shift(n)` answers a new array of the
  # first n. Both leave the rest behind, in this same array.
  def shift(*count)
    __check_frozen__
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

  # `[a, *b, c]` (#157). Appends the elements of `other` to this array and
  # answers this array, so the compiler can chain one call per piece.
  #
  # The splat conversion lives here rather than in the lowering because it is
  # Ruby's rule, not the compiler's: `*x` spreads what `x.to_a` gives, and
  # wraps anything that has no `to_a` in a one-element array. `nil.to_a` is
  # `[]`, which is why `[*nil]` is empty rather than `[nil]`.
  def __concat_splat__(other)
    spread = other.respond_to?(:to_a) ? other.to_a : [other]
    unless spread.is_a?(Array)
      raise TypeError, "can't convert " + other.class.to_s + " to Array"
    end
    i = 0
    while i < spread.size
      push(spread[i])
      i = i + 1
    end
    self
  end

  # A frozen Array refuses every mutator, including one that would not have
  # changed anything: `[1].freeze.concat([])` is a FrozenError. Measured.
  def __check_frozen__
    raise FrozenError, "can't modify frozen Array: " + inspect if frozen?
  end

  def concat(other)
    __check_frozen__
    other.each { |element| push(element) }
    self
  end

  # Nested Arrays join flat, with the same separator; an Array that holds
  # itself is an ArgumentError. The result starts US-ASCII and takes each
  # part's encoding as `<<` negotiates it, so incompatible parts raise
  # `Encoding::CompatibilityError`. Measured.
  def join(separator = "")
    separator = String.__coerce__(separator) unless separator.nil?
    out = "".b.__force_encoding__(2)
    __join_into__(out, separator, [], [true])
    out
  end

  # `first` is a one-element flag: CRuby copies the first part's encoding
  # onto the result before appending, so an ASCII-only UTF-8 first part
  # makes the whole join UTF-8.
  def __join_into__(out, separator, seen, first)
    raise ArgumentError, "recursive array join" if seen.any? { |outer| outer.equal?(self) }
    seen.push(self)
    i = 0
    while i < size
      out << separator if i > 0 && !separator.nil?
      item = self[i]
      if item.is_a?(Array)
        item.__join_into__(out, separator, seen, first)
      elsif !item.is_a?(String) && !item.respond_to?(:to_str) && item.respond_to?(:to_ary) &&
            (converted = item.to_ary).is_a?(Array)
        converted.__join_into__(out, separator, seen, first)
      else
        part = item.is_a?(String) ? item : (item.respond_to?(:to_str) ? item.to_str : item.to_s)
        if first[0]
          out.__force_encoding__(part.__encoding_index__)
          first[0] = false
        end
        out << part
      end
      i += 1
    end
    seen.pop
  end

  # In the first element's `inspect` encoding, negotiating as it appends; an
  # empty Array is US-ASCII, and one that holds itself shows `[...]` there.
  # Measured.
  def inspect
    return "[]".b.__force_encoding__(2) if empty?
    Kernel.__inspect_guard__(self, "[...]") do
      out = "[".b.__force_encoding__(2)
      i = 0
      while i < size
        part = self[i].inspect
        if i == 0
          out.__force_encoding__(part.__encoding_index__)
        else
          out << ", "
        end
        out << part
        i += 1
      end
      out << "]"
    end
  end

  def to_s
    inspect
  end
end

# The rest of `Array` (#21), all Ruby over `[]`, `[]=` and their slice forms.
# Every rule below was measured against ruby 4.0.7; the ones that are not
# obvious say so where they are applied.
class Array
  # `Array[1, 2]`, and `MyArray[1, 2]`, which answers an instance of the
  # subclass.
  def self.[](*items)
    made = allocate
    items.each { |item| made.push(item) }
    made
  end

  # An Array as is, anything with `to_ary` through it, nil otherwise. A
  # `to_ary` that answers something else is a TypeError naming both classes.
  def self.try_convert(value)
    return value if value.is_a?(Array)
    return nil unless value.respond_to?(:to_ary)
    converted = value.to_ary
    return converted if converted.nil? || converted.is_a?(Array)
    raise TypeError,
          "can't convert #{value.class} to Array (#{value.class}#to_ary gives #{converted.class})"
  end

  # An operand that has to be an Array: itself, or its `to_ary`.
  def self.__coerce__(value)
    return value if value.is_a?(Array)
    unless value.respond_to?(:to_ary)
      raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into Array"
    end
    value.to_ary
  end

  # --- set arithmetic -------------------------------------------------------
  # Membership is `hash` and `eql?`, a Hash's rule, which is what keeps `1` and
  # `1.0` apart here as they are as keys.

  def __member_table__(arrays)
    table = {}
    arrays.each { |array| array.each { |element| table[element] = true } }
    table
  end

  def -(other)
    exclude = __member_table__([Array.__coerce__(other)])
    reject { |element| exclude.key?(element) }
  end

  def difference(*others)
    exclude = __member_table__(others.map { |other| Array.__coerce__(other) })
    reject { |element| exclude.key?(element) }
  end

  def &(other)
    intersection(other)
  end

  def intersection(*others)
    others = others.map { |other| Array.__coerce__(other) }
    tables = others.map { |other| __member_table__([other]) }
    seen = {}
    out = []
    each do |element|
      next if seen.key?(element)
      next unless tables.all? { |table| table.key?(element) }
      seen[element] = true
      out.push(element)
    end
    out
  end

  def intersect?(other)
    table = __member_table__([Array.__coerce__(other)])
    any? { |element| table.key?(element) }
  end

  def |(other)
    union(other)
  end

  def union(*others)
    seen = {}
    out = []
    ([self] + others.map { |other| Array.__coerce__(other) }).each do |array|
      array.each do |element|
        next if seen.key?(element)
        seen[element] = true
        out.push(element)
      end
    end
    out
  end

  # A String joins, an Integer repeats; nothing else, and a negative count is
  # an ArgumentError. Either way the answer is a plain Array.
  def *(operand)
    return join(operand) if operand.is_a?(String)
    return join(operand.to_str) if operand.respond_to?(:to_str)
    count = operand
    unless count.is_a?(Integer)
      unless count.respond_to?(:to_int)
        raise TypeError, "no implicit conversion of #{count.nil? ? "nil" : count.class} into Integer"
      end
      count = count.to_int
    end
    raise ArgumentError, "negative argument" if count < 0
    out = []
    count.times { each { |element| out.push(element) } }
    out
  end

  # --- rewriting in place ---------------------------------------------------

  # Put `elements` where this array's contents were, keeping the object.
  def __refill__(elements)
    clear
    elements.each { |element| push(element) }
    self
  end

  def replace(other)
    __check_frozen__
    other = Array.__coerce__(other)
    return self if other.equal?(self)
    __refill__(other.dup)
  end

  def map!
    return to_enum(:map!) { size } unless block_given?
    __check_frozen__
    i = 0
    while i < size
      self[i] = yield(self[i])
      i = i + 1
    end
    self
  end

  def collect!(&block)
    return to_enum(:collect!) { size } if block.nil?
    map!(&block)
  end

  # Keep what the block says to keep. `select!`/`filter!` and `reject!` answer
  # nil when nothing changed; `keep_if` and `delete_if` always answer self.
  #
  # In place and as it goes, as CRuby's `ary_reject_bang` is: if the block
  # raises part-way, what was already dropped stays dropped and the rest —
  # the element that raised included — stays. Measured.
  def __keep__(keep)
    before = size
    read = 0
    write = 0
    begin
      while read < size
        element = self[read]
        verdict = yield(element) ? true : false
        read = read + 1
        if verdict == keep
          self[write] = element
          write = write + 1
        end
      end
    ensure
      self[write, size - write] = __take__(read, size - read)
    end
    size != before
  end

  def select!(&block)
    return to_enum(:select!) { size } if block.nil?
    __check_frozen__
    __keep__(true, &block) ? self : nil
  end

  def filter!(&block)
    return to_enum(:filter!) { size } if block.nil?
    select!(&block)
  end

  def keep_if(&block)
    return to_enum(:keep_if) { size } if block.nil?
    __check_frozen__
    __keep__(true, &block)
    self
  end

  def reject!(&block)
    return to_enum(:reject!) { size } if block.nil?
    __check_frozen__
    __keep__(false, &block) ? self : nil
  end

  def delete_if(&block)
    return to_enum(:delete_if) { size } if block.nil?
    __check_frozen__
    __keep__(false, &block)
    self
  end

  # Every element `==` to `value` goes. The answer is the last element that
  # went — the array's own, not the argument — or the block's value, or nil.
  def delete(value)
    found = nil
    hit = false
    kept = []
    each do |element|
      if element == value
        found = element
        hit = true
      else
        kept.push(element)
      end
    end
    unless hit
      return yield(value) if block_given?
      return nil
    end
    __check_frozen__
    __refill__(kept)
    found
  end

  def delete_at(index)
    __check_frozen__
    index = __count_like__(index)
    index = index + size if index < 0
    return nil if index < 0 || index >= size
    found = self[index]
    self[index, 1] = []
    found
  end

  def compact
    reject { |element| element.nil? }
  end

  def compact!
    __check_frozen__
    __keep__(false) { |element| element.nil? } ? self : nil
  end

  def uniq!(&block)
    __check_frozen__
    kept = uniq(&block)
    return nil if kept.size == size
    __refill__(kept)
  end

  def reverse!
    __check_frozen__
    __refill__(reverse)
  end

  def sort!(&block)
    __check_frozen__
    __refill__(sort(&block))
  end

  def sort_by!(&block)
    return to_enum(:sort_by!) { size } if block.nil?
    __check_frozen__
    __refill__(sort_by(&block))
  end

  def rotate(count = 1)
    count = __count_like__(count)
    return [] if empty?
    shift = count % size
    __take__(shift, size) + __take__(0, shift)
  end

  def rotate!(count = 1)
    __check_frozen__
    count = __count_like__(count)
    return self if empty?
    __refill__(rotate(count))
  end

  # `-1` is after the last element and a negative index counts from there, so
  # the smallest allowed is `-(size + 1)`. Past the end pads with nil.
  def insert(index, *items)
    __check_frozen__
    index = __count_like__(index)
    return self if items.empty?
    if index < 0
      at = index + size + 1
      if at < 0
        raise IndexError, "index #{index} too small for array; minimum: -#{size + 1}"
      end
      index = at
    end
    self[index, 0] = items
    self
  end

  # `fill(value)`, `fill(value, start, length)`, `fill(value, range)` and the
  # same three with a block taking the index instead of a value. A start before
  # the front clamps to 0; a range starting before it is a RangeError.
  def fill(*given)
    __check_frozen__
    if block_given?
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..2)" if given.size > 2
      value = nil
      bounds = given
    else
      raise ArgumentError, "wrong number of arguments (given 0, expected 1..3)" if given.empty?
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 1..3)" if given.size > 3
      value = given[0]
      bounds = given.__take__(1, 2)
    end
    start = 0
    count = nil
    if bounds.size == 1 && bounds[0].is_a?(Range)
      range = bounds[0]
      start = range.begin.nil? ? 0 : __count_like__(range.begin)
      start = start + size if start < 0
      raise RangeError, "#{range.inspect} out of range" if start < 0
      if range.end.nil?
        count = size - start
      else
        stop = __count_like__(range.end)
        stop = stop + size if stop < 0
        stop = stop + 1 unless range.exclude_end?
        count = stop - start
      end
    else
      unless bounds.empty? || bounds[0].nil?
        start = __count_like__(bounds[0])
        start = start + size if start < 0
        start = 0 if start < 0
      end
      count = __count_like__(bounds[1]) if bounds.size > 1 && !bounds[1].nil?
      count = size - start if count.nil?
    end
    return self if count <= 0
    i = start
    stop = start + count
    while i < stop
      self[i] = block_given? ? yield(i) : value
      i = i + 1
    end
    self
  end

  # An index argument: an Integer, or anything with `to_int`.
  def __count_like__(value)
    return value if value.is_a?(Integer)
    unless value.respond_to?(:to_int)
      raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into Integer"
    end
    value.to_int
  end

  # --- reading --------------------------------------------------------------

  def prepend(*items)
    unshift(*items)
  end

  def values_at(*selectors)
    out = []
    selectors.each do |selector|
      if selector.is_a?(Range)
        first = selector.begin.nil? ? 0 : __count_like__(selector.begin)
        first = first + size if first < 0
        raise RangeError, "#{selector.inspect} out of range" if first < 0
        last = selector.end.nil? ? size - 1 : __count_like__(selector.end)
        last = last + size if last < 0
        last = last - 1 if selector.exclude_end? && !selector.end.nil?
        i = first
        while i <= last
          out.push(self[i])
          i = i + 1
        end
      else
        out.push(self[__count_like__(selector)])
      end
    end
    out
  end

  def fetch(*given)
    if given.empty? || given.size > 2
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 1..2)"
    end
    index = __count_like__(given[0])
    at = index < 0 ? index + size : index
    return self[at] if at >= 0 && at < size
    return yield(index) if block_given?
    return given[1] if given.size == 2
    raise IndexError, "index #{index} outside of array bounds: #{-size}...#{size}"
  end

  def fetch_values(*indexes, &block)
    indexes.map { |index| block.nil? ? fetch(index) : fetch(index, &block) }
  end

  def dig(index, *rest)
    value = self[index]
    return value if rest.empty? || value.nil?
    unless value.respond_to?(:dig)
      raise TypeError, "#{value.class} does not have #dig method"
    end
    value.dig(*rest)
  end

  def assoc(key)
    each do |element|
      next unless element.is_a?(Array) || element.respond_to?(:to_ary)
      element = Array.__coerce__(element)
      return element if !element.empty? && element[0] == key
    end
    nil
  end

  def rassoc(value)
    each do |element|
      next unless element.is_a?(Array) || element.respond_to?(:to_ary)
      element = Array.__coerce__(element)
      return element if element.size > 1 && element[1] == value
    end
    nil
  end

  def rindex(*wanted)
    i = size - 1
    while i >= 0
      if wanted.empty?
        return to_enum(:rindex) unless block_given?
        return i if yield(self[i])
      elsif self[i] == wanted[0]
        return i
      end
      i = i - 1
      i = size - 1 if i >= size
    end
    nil
  end

  # `find` from the end (Ruby 4.0).
  #
  # The index is clamped to the array after every yield, so a block that
  # shrinks the array ends the walk rather than reading past it. Measured.
  def rfind(ifnone = nil)
    return to_enum(:rfind, ifnone) unless block_given?
    i = size - 1
    while i >= 0
      return self[i] if yield(self[i])
      i = i - 1
      i = size - 1 if i >= size
    end
    ifnone.nil? ? nil : ifnone.call
  end

  # From the end, CRuby's way: the position counts down and is clamped to the
  # array after each yield, so one that grows keeps its place and one that
  # shrinks is not read past.
  def reverse_each
    return to_enum(:reverse_each) { size } unless block_given?
    i = size
    while i > 0
      i = i - 1
      yield self[i]
      i = size if size < i
    end
    self
  end

  def transpose
    return [] if empty?
    rows = map { |row| Array.__coerce__(row) }
    width = rows[0].size
    rows.each do |row|
      unless row.size == width
        raise IndexError, "element size differs (#{row.size} should be #{width})"
      end
    end
    out = []
    width.times { |column| out.push(rows.map { |row| row[column] }) }
    out
  end

  # `level` deep, or all the way with a negative or no level. An Array that
  # contains itself cannot be flattened all the way: ArgumentError, measured.
  def flatten(level = -1)
    level = __count_like__(level) unless level.nil?
    level = -1 if level.nil?
    out = []
    __flatten_into__(out, self, level, [])
    out
  end

  def flatten!(level = -1)
    __check_frozen__
    level = __count_like__(level) unless level.nil?
    return nil if level == 0 || none? { |element| element.is_a?(Array) || element.respond_to?(:to_ary) }
    __refill__(flatten(level))
  end

  def __flatten_into__(out, array, level, open)
    if open.any? { |seen| seen.equal?(array) }
      raise ArgumentError, "tried to flatten recursive array"
    end
    open.push(array)
    array.each do |element|
      nested = element.is_a?(Array) ? element : (element.respond_to?(:to_ary) ? element.to_ary : nil)
      if !nested.nil? && level != 0
        __flatten_into__(out, nested, level - 1, open)
      else
        out.push(element)
      end
    end
    open.pop
  end

  # `slice!` is `slice`, and the part it answers is removed.
  def slice!(*given)
    __check_frozen__
    part = self[*given]
    return nil if part.nil?
    if given.size == 1 && !given[0].is_a?(Range)
      delete_at(given[0])
      return part
    end
    start = given[0].is_a?(Range) ? (given[0].begin || 0) : given[0]
    start = __count_like__(start)
    start = start + size if start < 0
    self[start, part.size] = []
    part
  end

  # --- search ---------------------------------------------------------------

  # Find-minimum mode when the block answers true/false/nil, find-any mode
  # when it answers a number; anything else is a TypeError. Measured wording.
  def bsearch(&block)
    return to_enum(:bsearch) if block.nil?
    at = bsearch_index(&block)
    at.nil? ? nil : self[at]
  end

  def bsearch_index
    return to_enum(:bsearch_index) unless block_given?
    low = 0
    high = size
    found = nil
    while low < high
      mid = low + (high - low) / 2
      verdict = yield(self[mid])
      if verdict == true
        found = mid
        high = mid
      elsif verdict.nil? || verdict == false
        low = mid + 1
      elsif verdict.is_a?(Integer) || verdict.is_a?(Float)
        return mid if verdict == 0
        if verdict < 0
          high = mid
        else
          low = mid + 1
        end
      else
        raise TypeError, "wrong argument type #{verdict.class} (must be numeric, true, false or nil)"
      end
    end
    found
  end

  # --- combinatorics ----------------------------------------------------------
  # Each takes a block or answers an Enumerator with its size, and iterates a
  # snapshot: measured, changing the array inside the block does not change
  # what is yielded.

  def permutation(*given, &block)
    count = given.empty? ? size : __count_like__(given[0])
    return to_enum(:permutation, *given) { __permutation_size__(count) } if block.nil?
    items = dup
    __permute__(items, count, [], Array.new(items.size, false), block) if count >= 0 && count <= items.size
    self
  end

  def __permutation_size__(count)
    return 0 if count < 0 || count > size
    total = 1
    i = 0
    while i < count
      total = total * (size - i)
      i = i + 1
    end
    total
  end

  def __permute__(items, count, chosen, used, block)
    if chosen.size == count
      block.call(chosen.dup)
      return
    end
    i = 0
    while i < items.size
      unless used[i]
        used[i] = true
        chosen.push(items[i])
        __permute__(items, count, chosen, used, block)
        chosen.pop
        used[i] = false
      end
      i = i + 1
    end
  end

  def combination(count, &block)
    count = __count_like__(count)
    return to_enum(:combination, count) { __choose__(size, count) } if block.nil?
    items = dup
    __combine__(items, count, 0, [], block) if count >= 0 && count <= items.size
    self
  end

  def __choose__(n, k)
    return 0 if k < 0 || k > n
    total = 1
    i = 0
    while i < k
      total = total * (n - i) / (i + 1)
      i = i + 1
    end
    total
  end

  def __combine__(items, count, from, chosen, block)
    if chosen.size == count
      block.call(chosen.dup)
      return
    end
    i = from
    while i < items.size
      chosen.push(items[i])
      __combine__(items, count, i + 1, chosen, block)
      chosen.pop
      i = i + 1
    end
  end

  def repeated_combination(count, &block)
    count = __count_like__(count)
    if block.nil?
      return to_enum(:repeated_combination, count) do
        count < 0 ? 0 : (count == 0 ? 1 : __choose__(size + count - 1, count))
      end
    end
    items = dup
    if count == 0
      block.call([])
    elsif count > 0 && !items.empty?
      __combine_again__(items, count, 0, [], block)
    end
    self
  end

  def __combine_again__(items, count, from, chosen, block)
    if chosen.size == count
      block.call(chosen.dup)
      return
    end
    i = from
    while i < items.size
      chosen.push(items[i])
      __combine_again__(items, count, i, chosen, block)
      chosen.pop
      i = i + 1
    end
  end

  def repeated_permutation(count, &block)
    count = __count_like__(count)
    return to_enum(:repeated_permutation, count) { count < 0 ? 0 : size**count } if block.nil?
    items = dup
    if count == 0
      block.call([])
    elsif count > 0 && !items.empty?
      __permute_again__(items, count, [], block)
    end
    self
  end

  def __permute_again__(items, count, chosen, block)
    if chosen.size == count
      block.call(chosen.dup)
      return
    end
    items.each do |item|
      chosen.push(item)
      __permute_again__(items, count, chosen, block)
      chosen.pop
    end
  end

  # With a block, each combination is yielded and the receiver answered;
  # without one, all of them as an Array.
  def product(*others, &block)
    lists = [dup] + others.map { |other| Array.__coerce__(other).dup }
    out = block.nil? ? [] : nil
    __product_into__(lists, 0, [], out, block)
    block.nil? ? out : self
  end

  def __product_into__(lists, depth, chosen, out, block)
    if depth == lists.size
      block.nil? ? out.push(chosen.dup) : block.call(chosen.dup)
      return
    end
    lists[depth].each do |item|
      chosen.push(item)
      __product_into__(lists, depth + 1, chosen, out, block)
      chosen.pop
    end
  end
end
