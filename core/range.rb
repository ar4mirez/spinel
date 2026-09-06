# Range.
#
# A range literal is compiled to `Range.new(begin, end, exclude_end)` (#157), so
# this class exists for the literal to have something to be. It holds the three
# things a literal carries and answers the questions that do not need iteration.
#
# ponytail: no `each`, `to_a`, `step`, `size`, or `sum`. Those need an iteration
# protocol over the begin value and belong to the `Range` core-library slice
# (#23); a call to one of them reports blocked, naming the method, which is how
# the next slice gets chosen.
class Range
  include Enumerable

  # `exclude_end` is positional and defaults to false, matching `Range.new`.
  #
  # The endpoints must be comparable, and Ruby checks that eagerly: `(1.."a")`
  # raises rather than waiting for someone to ask a question about it. A
  # beginless or endless range skips the check, because there is nothing to
  # compare against.
  def initialize(from, to, exclude_end = false)
    # A Range is frozen once built, so `r.send(:initialize, ...)` on a live one
    # is a mutation rather than a re-run. There is no `freeze` yet, so the flag
    # stands in for the frozen bit.
    #
    # ponytail: this catches `initialize` only. Real freezing is one header bit
    # and a check in `ivar_set`, and belongs with `Object#freeze`.
    raise FrozenError, "can't modify frozen Range: " + inspect if @initialized
    if !from.nil? && !to.nil? && (from <=> to).nil?
      raise ArgumentError, "bad value for range"
    end
    @begin = from
    @end = to
    @exclude_end = exclude_end ? true : false
    @initialized = true
    self
  end

  def begin
    @begin
  end

  def end
    @end
  end

  def exclude_end?
    @exclude_end
  end

  # `first` and `last` with no argument are the endpoints. With a count they
  # take from the sequence, which needs iteration — see the note on the class.
  def first(*count)
    raise RangeError, "cannot get the first element of beginless range" if @begin.nil?
    return @begin if count.empty?
    wanted = __to_count__(count[0])
    raise ArgumentError, "negative array size (or size too big)" if wanted < 0
    out = []
    return out if wanted == 0
    each do |value|
      out.push(value)
      break if out.size >= wanted
    end
    out
  end

  def last(*count)
    raise RangeError, "cannot get the last element of endless range" if @end.nil?
    return @end if count.empty?
    wanted = __to_count__(count[0])
    raise ArgumentError, "negative array size" if wanted < 0
    all = to_a
    from = all.size - wanted
    from = 0 if from < 0
    all.__take__(from, wanted)
  end

  # `first(n)` and `last(n)` take a count, and Ruby converts it with `to_int`
  # before it counts anything — `(1..5).first(nil)` is a TypeError, not an
  # ArgumentError about arity.
  def __to_count__(value)
    return value if value.is_a?(Integer)
    raise TypeError, "no implicit conversion from nil to integer" if value.nil?
    unless value.respond_to?(:to_int)
      raise TypeError, "no implicit conversion of " + value.class.to_s + " into Integer"
    end
    value.to_int
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
    unless @begin.nil?
      low = (@begin <=> value)
      return false if low.nil? || low > 0
    end
    return true if @end.nil?
    high = (value <=> @end)
    return false if high.nil?
    @exclude_end ? high < 0 : high <= 0
  end

  def ===(value)
    cover?(value)
  end

  # Iteration, by `succ` and `<=>`, which is the protocol Ruby uses. Only
  # `to_a` and the splat need it today; `step`, `map`, and the rest of
  # Enumerable are still #23's.
  #
  # ponytail: a begin whose class has no `succ` raises `TypeError`, which is
  # Ruby's answer for `(1.0..3.0)` and the wrong one for `("a".."c")` — Spinel
  # has no `String#succ` yet, and this cannot tell "no succ in Ruby" from "no
  # succ here". The upgrade is `String#succ`; nothing in this file changes.
  def each
    unless @begin.respond_to?(:succ)
      raise TypeError, "can't iterate from " + @begin.class.to_s
    end
    current = @begin
    while @end.nil? || __before_end__(current)
      yield current
      current = current.succ
    end
    self
  end

  def to_a
    raise RangeError, "cannot convert endless range to an array" if @end.nil?
    out = []
    each { |value| out.push(value) }
    out
  end

  def entries
    to_a
  end

  # Whether iteration has not yet passed the end. Separate because `each` asks
  # it once per step and the exclusive flag is the only difference.
  def __before_end__(value)
    cmp = (value <=> @end)
    return false if cmp.nil?
    @exclude_end ? cmp < 0 : cmp <= 0
  end

  def to_s
    left = @begin.nil? ? "" : @begin.to_s
    right = @end.nil? ? "" : @end.to_s
    left + (@exclude_end ? "..." : "..") + right
  end

  # `inspect` writes an omitted endpoint as nothing — `(1..)` is `"1.."` — but
  # a range with *both* ends nil as `"nil..nil"`, because `".."` alone would not
  # read as a range at all. Measured from CRuby, not reasoned about.
  def inspect
    return "nil" + (@exclude_end ? "..." : "..") + "nil" if @begin.nil? && @end.nil?
    left = @begin.nil? ? "" : @begin.inspect
    right = @end.nil? ? "" : @end.inspect
    left + (@exclude_end ? "..." : "..") + right
  end
end
