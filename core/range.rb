# Range.
#
# A range literal is compiled to `Range.new(begin, end, exclude_end)` (#157), so
# this class exists for the literal to have something to be. It holds the three
# things a literal carries and answers the questions that do not need iteration.
#
# `include Enumerable` supplies most of the rest. `min`, `max`, `count`,
# `reverse_each` and `include?` are Range's own because at an open end the
# answer is a rule rather than a search, and a generic scan would either guess
# or run forever. `step` and `%` are in `core/arithmetic_sequence.rb`, beside
# the sequence they build.
#
# Most methods here are ports of CRuby's `range.c`, because which comparisons
# and conversions they make, in which order, is what ruby/spec checks.
class Range
  include Enumerable

  # `exclude_end` is positional and defaults to false, matching `Range.new`.
  #
  # The endpoints must be comparable, and Ruby checks that eagerly: `(1.."a")`
  # raises rather than waiting for someone to ask a question about it. A
  # beginless or endless range skips the check, because there is nothing to
  # compare against.
  def initialize(from, to, exclude_end = false)
    # Running `initialize` a second time is a FrozenError even on a subclass
    # instance that is not frozen — measured — so the flag stays beside the
    # real frozen bit below.
    raise FrozenError.new("can't modify frozen Range: " + inspect, receiver: self) if @__initialized__
    if !from.nil? && !to.nil? && (from <=> to).nil?
      raise ArgumentError, "bad value for range"
    end
    @__begin__ = from
    @__end__ = to
    @__exclude_end__ = exclude_end ? true : false
    @__initialized__ = true
    # A Range is frozen once built — a subclass's instances are not.
    # Measured: `(1..2).frozen?` and `Range.new(1, 2).frozen?` are true.
    freeze if instance_of?(Range)
    self
  end

  def begin
    @__begin__
  end

  def end
    @__end__
  end

  def exclude_end?
    @__exclude_end__
  end

  # `first` and `last` with no argument are the endpoints. With a count they
  # take from the sequence, which needs iteration — see the note on the class.
  def first(*count)
    raise RangeError, "cannot get the first element of beginless range" if @__begin__.nil?
    return @__begin__ if count.empty?
    Range.__arity__(count, 1)
    __take_from__(:each, count[0])
  end

  # CRuby's `first_i` loop, shared by `first(n)` and `max(n)`: the count is
  # converted before anything is walked — `(1..5).first(nil)` is a TypeError,
  # not an ArgumentError about arity — and the walk starts even for a count of
  # zero, so `(1.0..2.0).first(0)` still says Float cannot be iterated.
  def __take_from__(walk, count)
    wanted = Integer.__index__(count)
    raise ArgumentError, "negative array size (or size too big)" if wanted < 0
    out = []
    __send__(walk) do |value|
      break if out.size >= wanted
      out.push(value)
    end
    out
  end

  # An Integer range's last n are counted back from the end, without the walk
  # `to_a` would make — `(0...2**64).last(2)` answers. CRuby's
  # `rb_int_range_last`, and only while `each` is Range's own.
  def last(*count)
    raise RangeError, "cannot get the last element of endless range" if @__end__.nil?
    return @__end__ if count.empty?
    Range.__arity__(count, 1)
    from = @__begin__
    to = @__end__
    unless (from.nil? || from.is_a?(Integer)) && to.is_a?(Integer) &&
           method(:each).owner.equal?(Range)
      return to_a.last(count[0])
    end
    to = to - 1 if @__exclude_end__
    length = from.nil? ? nil : to - from + 1
    return [] if !length.nil? && length <= 0
    wanted = Integer.__index__(count[0])
    raise ArgumentError, "negative array size" if wanted < 0
    wanted = length if !length.nil? && wanted > length
    out = []
    value = to - wanted
    while out.size < wanted
      value = value + 1
      out.push(value)
    end
    out
  end

  # `rb_scan_args` with one optional argument, for the methods that take
  # their count through `*count` so they can tell "none" from nil.
  def self.__arity__(given, most)
    return if given.size <= most
    raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..#{most})"
  end

  def ==(other)
    return false unless other.is_a?(Range)
    self.begin == other.begin && self.end == other.end &&
      exclude_end? == other.exclude_end?
  end

  def eql?(other)
    return false unless other.is_a?(Range)
    self.begin.eql?(other.begin) && self.end.eql?(other.end) &&
      exclude_end? == other.exclude_end?
  end

  # `cover?` is the comparison-based test, and it is what `===` uses. An
  # endpoint that is `nil` is unbounded on that side rather than a value to
  # compare against, which is what makes `(1..)` cover every Integer above 1.
  def cover?(value)
    return __cover_range__(value) if value.is_a?(Range)
    __covers__(value)
  end

  # CRuby's `r_cover_p`.
  def __covers__(value)
    return false unless @__begin__.nil? || Range.__less__(@__begin__, value) <= 0
    return true if @__end__.nil?
    Range.__less__(value, @__end__) <= (@__exclude_end__ ? -1 : 0)
  end

  # CRuby's `r_cover_range_p`: every element of `other` is in this range.
  # Ported, because which comparisons it makes in which order is what the
  # spec checks.
  def __cover_range__(other)
    low = other.begin
    high = other.end
    return false if !@__end__.nil? && high.nil?
    return false if !@__begin__.nil? && low.nil?
    if !low.nil? && !high.nil? && Range.__less__(low, high) > (other.exclude_end? ? -1 : 0)
      return false
    end
    return false if !low.nil? && !__covers__(low)
    compared = Range.__less__(@__end__, high)
    if @__exclude_end__ == other.exclude_end?
      return compared >= 0
    elsif @__exclude_end__
      return compared > 0
    elsif compared >= 0
      return true
    end
    # An inclusive self against an exclusive other ending past it: what
    # matters is other's last element, if it has one.
    last = begin
      other.max
    rescue TypeError
      nil
    end
    return false if last.nil?
    Range.__less__(@__end__, last) >= 0
  end

  # CRuby's `r_less`: `<=>` as an Integer, and incomparable as greater than
  # anything, so a check that needs "less" fails.
  def self.__less__(a, b)
    result = a <=> b
    return 2**31 - 1 if result.nil?
    result <=> 0
  end

  def ===(value)
    cover?(value)
  end

  # CRuby's `range_hash`: the endpoints' own hashes and the exclusive flag, so
  # two ranges that are `eql?` hash alike.
  def hash
    flag = @__exclude_end__ ? 1 : 0
    digest = __hash_combine__(:__spinel_range__, flag)
    digest = __hash_combine__(digest, __element_hash__(@__begin__))
    digest = __hash_combine__(digest, __element_hash__(@__end__))
    __hash_combine__(digest, flag << 24)
  end

  # CRuby's `range_overlap`. Only an empty range is ruled out by its own
  # endpoints; a beginless one is never assumed empty, whatever its end.
  def overlap?(other)
    unless other.is_a?(Range)
      raise TypeError, "wrong argument type #{other.class} (expected Range)"
    end
    from = @__begin__
    to = @__end__
    other_from = other.begin
    other_to = other.end
    return false if Range.__empty_region__?(from, other_to, other.exclude_end?)
    return false if Range.__empty_region__?(other_from, to, @__exclude_end__)
    if !from.nil? && !other_from.nil?
      order = from <=> other_from
      return false if order.nil?
      return true if Range.__less__(from, other_from) == 0
    elsif from.nil? && !to.nil? && other_from.nil? && !other_to.nil?
      return !(to <=> other_to).nil?
    end
    return false if Range.__empty_region__?(from, to, @__exclude_end__)
    return false if Range.__empty_region__?(other_from, other_to, other.exclude_end?)
    true
  end

  def self.__empty_region__?(from, to, exclude)
    return false if from.nil? || to.nil?
    less = __less__(from, to)
    less > 0 || (exclude && less == 0)
  end

  # `count` with neither an argument nor a block is a size, never a walk: an
  # open end is infinitely many, measured as CRuby's `range_count` answers it.
  def count(*item, &block)
    return super unless item.empty? && block.nil?
    return Float::INFINITY if @__begin__.nil? || @__end__.nil?
    if Range.__integer_like__?(@__begin__)
      known = size
      return known unless known.nil?
    end
    super
  end

  def to_set(*given, &block)
    raise RangeError, "cannot convert endless range to a set" if @__end__.nil?
    super
  end

  # CRuby's `is_integer_p`: an Integer, or anything whose `integer?` says so.
  def self.__integer_like__?(value)
    return true if value.is_a?(Integer)
    value.respond_to?(:integer?) && value.integer? ? true : false
  end

  # The element count without iterating. CRuby's `range_size` — the *begin*
  # decides, and it decides three different ways:
  #
  #   Integer begin      the count, or `Float::INFINITY` when the range is endless
  #   no `succ`          TypeError "can't iterate from <Class>" — `(1.0..2.0)`,
  #                      `(..1)` and `(Time.now..)` all raise
  #   anything else      nil, because a generic Range has no length —
  #                      `("a".."c").size` is nil, not an error
  #
  # A Float *end* against an Integer begin is 2 for `(1..2.5)`: the end is
  # floored, and `exclude_end?` only takes one off when the end is a whole
  # number, so `(1...2.5)` is also 2 while `(1...3.0)` is 2 rather than 3.
  def size
    from = @__begin__
    unless from.is_a?(Integer) && (@__end__.nil? || @__end__.is_a?(Numeric))
      unless from.respond_to?(:succ)
        raise TypeError, "can't iterate from " + from.class.to_s
      end
      return nil
    end
    return Float::INFINITY if @__end__.nil?
    stop = @__end__
    unless stop.is_a?(Integer)
      # An infinite end has no floor; `ruby_float_step_size` counts it as
      # infinitely many steps forward and none backward.
      if stop.is_a?(Float) && stop.infinite?
        return stop > 0 ? Float::INFINITY : 0
      end
      stop = stop.floor
      stop = stop - 1 if @__exclude_end__ && stop == @__end__
      return stop < from ? 0 : stop - from + 1
    end
    stop = stop - 1 if @__exclude_end__
    stop < from ? 0 : stop - from + 1
  end

  # Iteration, by `succ` and `<=>`, which is the protocol Ruby uses. A String
  # or Symbol range walks as `String#upto` does — `(:A..:z)` is 58 symbols,
  # where `succ` alone would run from `:Z` on to `:AA` and never reach `:z`.
  #
  # An inclusive walk stops on reaching the end, without asking the end for its
  # `succ`: CRuby's `range_each_func`, and what lets a Time with a `succ` of its
  # own end on one that has none.
  def each
    return to_enum(:each) { size } unless block_given?
    from = @__begin__
    to = @__end__
    if from.is_a?(Symbol) && (to.nil? || to.is_a?(Symbol))
      unless to.nil?
        from.to_s.upto(to.to_s, @__exclude_end__) { |name| yield name.to_sym }
        return self
      end
    elsif from.is_a?(String) && to.is_a?(String)
      from.upto(to, @__exclude_end__) { |text| yield text }
      return self
    end
    unless from.respond_to?(:succ)
      raise TypeError, "can't iterate from " + from.class.to_s
    end
    current = from
    if to.nil?
      while true
        yield current
        current = current.succ
      end
    end
    while true
      order = Range.__less__(current, to)
      break if order > 0 || (@__exclude_end__ && order == 0)
      yield current
      break if order == 0
      current = current.succ
    end
    self
  end

  def to_a
    raise RangeError, "cannot convert endless range to an array" if @__end__.nil?
    out = []
    each { |value| out.push(value) }
    out
  end

  def entries
    to_a
  end

  # Range answers `min`, `max`, `reverse_each` and `include?` itself rather than
  # letting Enumerable walk it, because at an open end the answer is a rule and
  # not a search: the minimum of a beginless range is an error, not a scan, and
  # Enumerable would either guess or iterate forever. CRuby's `range_min` and
  # `range_max`.

  def min(*count, &block)
    raise RangeError, "cannot get the minimum of beginless range" if @__begin__.nil?
    unless block.nil?
      if @__end__.nil?
        raise RangeError, "cannot get the minimum of endless range with custom comparison method"
      end
      return super
    end
    # The smallest n of an ascending walk are its first n.
    return first(*count) unless count.empty?
    return @__begin__ if @__end__.nil?
    order = Range.__cmp__(@__begin__, @__end__)
    return nil if order > 0 || (order == 0 && @__exclude_end__)
    @__begin__
  end

  def max(*count, &block)
    to = @__end__
    raise RangeError, "cannot get the maximum of endless range" if to.nil?
    from = @__begin__
    # An excluded end that is not a number is found by walking.
    if !block.nil? || (@__exclude_end__ && !to.is_a?(Numeric))
      if from.nil?
        raise RangeError,
              "cannot get the maximum of beginless range with custom comparison method"
      end
      return super
    end
    unless count.empty?
      # The largest n of an ascending walk are the first n of it reversed.
      Range.__arity__(count, 1)
      return __take_from__(:reverse_each, count[0])
    end
    order = from.nil? ? -1 : Range.__cmp__(from, to)
    return nil if order > 0
    return to unless @__exclude_end__
    raise TypeError, "cannot exclude non Integer end value" unless to.is_a?(Integer)
    return nil if order == 0
    unless from.nil? || from.is_a?(Integer)
      raise TypeError, "cannot exclude end value with non Integer begin value"
    end
    to - 1
  end

  # With a block, one walk feeds both answers — Enumerable's `min` then `max`
  # would ask every element for its `succ` twice. Without one, CRuby's
  # `range_minmax` is the two rules above, and walks nothing.
  def minmax(&block)
    return [min, max] if block.nil?
    members = to_a
    [members.min(&block), members.max(&block)]
  end

  # `OPTIMIZED_CMP`: `<=>` read as `rb_cmpint` reads it, so an answer of nil is
  # the comparison failing rather than a quiet false.
  def self.__cmp__(a, b)
    order = a <=> b
    if order.nil?
      raise ArgumentError, "comparison of #{a.class} with #{b.inspect} failed"
    end
    return order <=> 0 if order.is_a?(Integer)
    return 1 if order > 0
    return -1 if order < 0
    0
  end

  # Walking backwards needs an end to walk back from, so an endless range is a
  # TypeError naming the nil — not the RangeError `to_a` would raise. An
  # Integer end is counted down, beginless or not; anything else is
  # Enumerable's walk forwards, reversed. CRuby's `range_reverse_each`.
  def reverse_each(&block)
    return Enumerator.__for__(self, :reverse_each, [], proc { __reverse_size__ }) if block.nil?
    to = @__end__
    raise TypeError, "can't iterate from NilClass" if to.nil?
    from = @__begin__
    if (from.nil? || from.is_a?(Integer)) && to.is_a?(Integer)
      to = to - 1 if @__exclude_end__
      while from.nil? || to >= from
        block.call(to)
        to = to - 1
      end
      return self
    end
    super
  end

  # The size a reversed range reports, and the errors asking for it raises.
  # CRuby's `range_reverse_size`: an Integer begin counts as `size` does, a
  # beginless Integer range is infinite, and a range that cannot be walked
  # backwards says so naming the *end's* class — `(1.1..3)` raises "can't
  # iterate from Integer". Measured; the class in the message is not the one
  # that caused the problem.
  def __reverse_size__
    from = @__begin__
    to = @__end__
    raise TypeError, "can't iterate from NilClass" if to.nil?
    if from.is_a?(Integer)
      return size if to.is_a?(Numeric)
      raise TypeError, "can't iterate from " + to.class.to_s
    end
    if from.nil?
      return Float::INFINITY if to.is_a?(Integer)
      raise TypeError, "can't iterate from " + to.class.to_s
    end
    raise TypeError, "can't iterate from " + to.class.to_s unless from.respond_to?(:succ)
    nil
  end

  # `include?` is `cover?` for the ranges Ruby treats as linear — numbers and
  # Times — and a walk for everything else, which an open end makes
  # impossible. CRuby's `range_include_internal`. A Range argument is compared
  # as a value, not covered: `(1..10).include?(2..3)` is false.
  def include?(value)
    from = @__begin__
    to = @__end__
    if Range.__linear__?(from) || Range.__linear__?(to) ||
       Range.__to_integer__?(from) || Range.__to_integer__?(to)
      return __covers__(value)
    end
    return __string_include__(value) if from.is_a?(String) && to.is_a?(String)
    return true if from.nil? && to.nil? && Range.__linear__?(value)
    if from.nil? || to.nil?
      raise TypeError, "cannot determine inclusion in beginless/endless ranges"
    end
    each { |member| return true if member == value }
    false
  end

  def member?(value)
    include?(value)
  end

  # CRuby's `rb_str_include_range_p`: the argument as a String (anything
  # that does not convert is simply not a member), then the `upto` walk.
  def __string_include__(value)
    unless value.is_a?(String)
      return false unless value.respond_to?(:to_str)
      converted = value.to_str
      unless converted.is_a?(String)
        raise TypeError, "can't convert #{value.class} to String (#{value.class}#to_str gives #{converted.class})"
      end
      value = converted
    end
    @__begin__.upto(@__end__, exclude_end?) { |member| return true if member == value }
    false
  end

  # CRuby's `linear_object_p`: a number or a Time, whose members are every
  # value between the ends rather than the ones `succ` reaches.
  def self.__linear__?(value)
    value.is_a?(Numeric) || value.is_a?(Time)
  end

  # `rb_check_to_integer`: whether `to_int` answers an Integer. An end that
  # converts counts as a number, measured.
  def self.__to_integer__?(value)
    return false unless value.respond_to?(:to_int)
    value.to_int.is_a?(Integer)
  end

  # CRuby's `range_bsearch`. Integers are bisected directly, Floats through
  # their bit patterns — read as signed 64-bit integers, which order the same
  # way the Floats do — and an open Integer end is found first by doubling a
  # stride until the block's answer changes sign.
  def bsearch(&block)
    from = @__begin__
    to = @__end__
    if Range.__fixnum__?(from) && Range.__fixnum__?(to)
      return to_enum(:bsearch) if block.nil?
      return Range.__bisect__(from, to, @__exclude_end__, true, nil, block)
    elsif from.is_a?(Float) || to.is_a?(Float)
      low = Range.__float_bits__(from.nil? ? -Float::INFINITY : Float(from))
      high = Range.__float_bits__(to.nil? ? Float::INFINITY : Float(to))
      return to_enum(:bsearch) if block.nil?
      return Range.__bisect__(low, high, @__exclude_end__, true, nil, block) do |bits|
        Range.__bits_float__(bits)
      end
    elsif Range.__integer_like__?(from) && Range.__integer_like__?(to)
      return to_enum(:bsearch) if block.nil?
      return Range.__bisect__(Integer.__to_int__(from), Integer.__to_int__(to),
                              @__exclude_end__, false, nil, block)
    elsif Range.__integer_like__?(from) && to.nil?
      return to_enum(:bsearch) if block.nil?
      stride = 1
      satisfied = nil
      while true
        probe = from + stride
        verdict = Range.__bsearch_verdict__(block.call(probe))
        return probe if verdict == :hit
        satisfied = probe if verdict == :satisfied
        if verdict != :larger
          fixnums = Range.__fixnum__?(from) && Range.__fixnum__?(probe)
          return Range.__bisect__(from, probe, false, fixnums, fixnums ? satisfied : nil, block)
        end
        stride = stride * 2
        from = probe
      end
    elsif from.nil? && Range.__integer_like__?(to)
      return to_enum(:bsearch) if block.nil?
      stride = -1
      satisfied = nil
      while true
        probe = to + stride
        verdict = Range.__bsearch_verdict__(block.call(probe))
        return probe if verdict == :hit
        satisfied = probe if verdict == :satisfied
        if verdict == :larger
          fixnums = Range.__fixnum__?(probe) && Range.__fixnum__?(to)
          return Range.__bisect__(probe, to, false, fixnums, fixnums ? satisfied : nil, block)
        end
        stride = stride * 2
        to = probe
      end
    end
    raise TypeError, "can't do binary search for #{from.class}"
  end

  # The half-open interval (low, high] holds the target; it shrinks until the
  # two are adjacent. `convert` turns a point into the value the block sees.
  # A fixnum midpoint rounds toward zero, as C's division does, which decides
  # which of several zero answers is found; a bignum one rounds down, as
  # `Integer#div` does.
  def self.__bisect__(low, high, exclude, fixnums, satisfied, block, &convert)
    high = high + 1 unless exclude
    low = low - 1
    while low + 1 < high
      if !fixnums || (high < 0) == (low < 0)
        mid = fixnums ? low + (high - low) / 2 : (high + low).div(2)
      else
        sum = low + high
        mid = sum < 0 ? -(-sum / 2) : sum / 2
      end
      value = convert.nil? ? mid : convert.call(mid)
      verdict = __bsearch_verdict__(block.call(value))
      return value if verdict == :hit
      satisfied = value if verdict == :satisfied
      if verdict == :larger
        low = mid
      else
        high = mid
      end
    end
    satisfied
  end

  # CRuby's `BSEARCH_CHECK`: what the block's answer says about the target.
  # :hit is found, :satisfied and :smaller are at or below, :larger above.
  def self.__bsearch_verdict__(answer)
    if answer.is_a?(Integer)
      return :hit if answer == 0
      return answer < 0 ? :smaller : :larger
    end
    return :satisfied if true.equal?(answer)
    return :larger unless answer
    if answer.is_a?(Numeric)
      order = __cmp__(answer, 0)
      return :hit if order == 0
      return order < 0 ? :smaller : :larger
    end
    raise TypeError, "wrong argument type #{answer.class} (must be numeric, true, false or nil)"
  end

  def self.__fixnum__?(value)
    value.is_a?(Integer) && value >= -4611686018427387904 && value <= 4611686018427387903
  end

  # `double_as_int64`: a Float's bits as a signed integer, negated for a
  # negative Float so the order is kept. `-0.0` maps where `0.0` does, so a
  # search never yields it.
  def self.__float_bits__(value)
    bits = [value.abs].pack("D").unpack1("q")
    value < 0 ? -bits : bits
  end

  def self.__bits_float__(bits)
    return -([-bits].pack("q").unpack1("D")) if bits < 0
    [bits].pack("q").unpack1("D")
  end

  def to_s
    left = @__begin__.nil? ? "" : @__begin__.to_s
    right = @__end__.nil? ? "" : @__end__.to_s
    left + (@__exclude_end__ ? "..." : "..") + right
  end

  # `inspect` writes an omitted endpoint as nothing — `(1..)` is `"1.."` — but
  # a range with *both* ends nil as `"nil..nil"`, because `".."` alone would not
  # read as a range at all. Measured from CRuby, not reasoned about.
  def inspect
    return "nil" + (@__exclude_end__ ? "..." : "..") + "nil" if @__begin__.nil? && @__end__.nil?
    left = @__begin__.nil? ? "" : @__begin__.inspect
    right = @__end__.nil? ? "" : @__end__.inspect
    left + (@__exclude_end__ ? "..." : "..") + right
  end
end
