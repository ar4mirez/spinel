# Range.
#
# A range literal is compiled to `Range.new(begin, end, exclude_end)` (#157), so
# this class exists for the literal to have something to be. It holds the three
# things a literal carries and answers the questions that do not need iteration.
#
# `include Enumerable` supplies most of the rest. The four methods below it —
# `min`, `max`, `reverse_each` and `include?` — are Range's own because at an
# open end the answer is a rule rather than a search, and a generic scan would
# either guess or run forever.
#
# ponytail: no `step` or `size`. Both need an iteration protocol richer than
# `succ` — a numeric stride, and a count that does not walk — and belong to the
# `Range` slice (#23); a call to one reports blocked, naming the method, which
# is how the next slice gets chosen.
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

  # The element count without iterating. Ruby 3.4's rule, measured rather than
  # reasoned about — the *begin* decides, and it decides three different ways:
  #
  #   Integer begin      the count, or `Float::INFINITY` when the range is endless
  #   Numeric or nil     TypeError "can't iterate from <Class>" — `(1.0..2.0)`,
  #                      `(..1)` and `(1r..2r)` all raise
  #   anything else      nil, because a generic Range has no length —
  #                      `("a".."c").size` is nil, not an error
  #
  # A Float *end* against an Integer begin is 2 for `(1..2.5)`: the end is
  # floored, and `exclude_end?` only takes one off when the end is a whole
  # number, so `(1...2.5)` is also 2 while `(1...3.0)` is 2 rather than 3. That
  # arithmetic is written but not reachable yet — there is no float-to-integer
  # primitive, so `Float#floor` reports itself missing by name (#18).
  #
  # `Float::INFINITY` is mentioned rather than approximated: this VM has only
  # flonums and an infinity needs a heap `Float` (#18), so an endless range
  # reports that missing constant by name instead of answering a wrong number.
  def size
    from = @begin
    unless from.is_a?(Integer)
      if from.nil? || from.is_a?(Numeric)
        raise TypeError, "can't iterate from " + from.class.to_s
      end
      return nil
    end
    return Float::INFINITY if @end.nil?
    stop = @end
    unless stop.is_a?(Integer)
      return nil unless stop.is_a?(Numeric)
      stop = stop.floor
      stop = stop - 1 if @exclude_end && stop == @end
      return stop < from ? 0 : stop - from + 1
    end
    stop = stop - 1 if @exclude_end
    stop < from ? 0 : stop - from + 1
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

  # Range answers `min`, `max`, `reverse_each` and `include?` itself rather than
  # letting Enumerable walk it, because at an open end the answer is a rule and
  # not a search: the minimum of a beginless range is an error, not a scan, and
  # Enumerable would either guess or iterate forever. Every case below is
  # measured on ruby 4.0.6.

  def min(*count, &block)
    raise RangeError, "cannot get the minimum of beginless range" if @begin.nil?
    unless __has_members__?
      return count.empty? ? nil : []
    end
    return @begin if block.nil? && count.empty?
    if @end.nil?
      if block.nil? && count.size == 1
        # An endless range is already ascending, so its smallest n are its first
        # n — as long as it can be walked at all.
        unless @begin.respond_to?(:succ)
          raise TypeError, "can't iterate from " + @begin.class.to_s
        end
        return first(count[0])
      end
      raise RangeError, "cannot get the minimum of endless range with custom comparison method"
    end
    super
  end

  def max(*count, &block)
    raise RangeError, "cannot get the maximum of endless range" if @end.nil?
    if @begin.nil?
      unless block.nil?
        raise RangeError,
              "cannot get the maximum of beginless range with custom comparison method"
      end
      return __last_member__ if count.empty?
      # `max(n)` on a beginless range steps back from the end, so the end has to
      # be something that can be stepped. Measured: the error names the nil
      # begin, not the end that could not be decremented.
      raise TypeError, "can't iterate from NilClass" unless @end.is_a?(Integer)
      out = []
      value = __last_member__
      while out.size < count[0]
        out.push(value)
        value = value - 1
      end
      return out
    end
    unless __has_members__?
      return count.empty? ? nil : []
    end
    return __last_member__ if block.nil? && count.empty?
    super
  end

  # The largest member, which is the end itself unless the end is excluded — and
  # an excluded end can only be stepped back from when it is an Integer.
  def __last_member__
    return @end unless @exclude_end
    unless @end.is_a?(Integer)
      raise TypeError, "cannot exclude non Integer end value"
    end
    @end - 1
  end

  # Whether the range holds anything at all: `(3..1)` holds nothing.
  def __has_members__?
    return true if @begin.nil? || @end.nil?
    cmp = (@begin <=> @end)
    return false if cmp.nil?
    @exclude_end ? cmp < 0 : cmp <= 0
  end

  # Walking backwards needs an end to walk back from, so an endless range is a
  # TypeError naming the nil — not the RangeError `to_a` would raise.
  def reverse_each(&block)
    if @end.nil?
      raise TypeError, "can't iterate from NilClass"
    end
    return Enumerator.__for__(self, :reverse_each, [], proc { __reverse_size__ }) if block.nil?
    to_a.reverse.each { |value| block.call(value) }
    self
  end

  # The size a reversed range reports, and the errors asking for it raises.
  #
  # A non-numeric range has no size and says so with nil. A numeric one has to
  # be steppable backwards from an Integer end, and when it is not the TypeError
  # names the end's class — `(1.1..3)` raises "can't iterate from Integer".
  # Measured; the class in the message is not the one that caused the problem.
  def __reverse_size__
    raise TypeError, "can't iterate from NilClass" if @end.nil?
    return nil unless __numeric__?
    unless @begin.nil? || @begin.is_a?(Integer)
      raise TypeError, "can't iterate from " + @end.class.to_s
    end
    unless @end.is_a?(Integer)
      raise TypeError, "can't iterate from " + @end.class.to_s
    end
    return nil if @begin.nil?
    span = __last_member__ - @begin + 1
    span < 0 ? 0 : span
  end

  def __numeric__?
    return true if @begin.is_a?(Numeric)
    @end.is_a?(Numeric)
  end

  # `include?` is `cover?` for the ranges Ruby treats as linear — numbers — and
  # a walk for everything else, which an open end makes impossible.
  def include?(value)
    return cover?(value) if __linear__?
    if @begin.nil? || @end.nil?
      raise TypeError, "cannot determine inclusion in beginless/endless ranges"
    end
    each { |member| return true if member == value }
    false
  end

  def member?(value)
    include?(value)
  end

  def __linear__?
    return false if @begin.nil? && @end.nil?
    left = @begin.nil? || @begin.is_a?(Numeric)
    right = @end.nil? || @end.is_a?(Numeric)
    left && right
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
