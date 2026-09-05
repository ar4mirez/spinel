# Float.
#
# `to_s` is a primitive: printing the shortest decimal that reads back as the
# same float is an algorithm, not a formatting rule, and Ruby's answer is the
# one this has to match.
#
# Only flonums are Floats here. NaN, the infinities and `-0.0` need a heap box
# that this VM does not have yet, so `nan?` and `infinite?` are absent rather
# than answering `false` for values that cannot be made.
class Float
  def inspect
    to_s
  end

  def eql?(other)
    other.is_a?(Float) && self == other
  end

  def to_f
    self
  end

  def zero?
    self == 0.0
  end

  def positive?
    self > 0.0
  end

  def negative?
    self < 0.0
  end

  def abs
    self < 0.0 ? -self : self
  end

  def integer?
    false
  end

  # Ruby asks a non-Numeric to `coerce` itself before giving up, which is how
  # a user-defined numeric type compares against a Float at all. A misbehaving
  # `coerce` is a TypeError with Ruby's own wording, measured rather than
  # guessed — `comparison_spec.rb` asserts on the text.
  def <=>(other)
    unless other.is_a?(Numeric)
      return nil unless other.respond_to?(:coerce)
      pair = other.coerce(self)
      unless pair.is_a?(Array) && pair.size == 2
        raise TypeError, "coerce must return [x, y]"
      end
      return pair[0] <=> pair[1]
    end
    return -1 if self < other
    return 1 if self > other
    0
  end
end
