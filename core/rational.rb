# Rational — an exact fraction (#34).
#
# Two Integers, a numerator and a positive denominator with no common factor,
# so every Rational has one spelling and equality is comparing the two. The
# arithmetic is the schoolbook kind over Integers, which are unbounded here,
# so nothing is ever rounded. It is all Ruby.
#
# `Rational.new` does not exist, in Ruby or here: `Kernel#Rational` is the
# way in, and `Rational.__make__` is what it and the arithmetic call.
class Rational < Numeric
  class << self
    undef_method :new

    # `numerator / denominator`, reduced. The denominator is not zero.
    def __make__(numerator, denominator)
      if denominator < 0
        numerator = -numerator
        denominator = -denominator
      end
      common = numerator.gcd(denominator)
      unless common == 1
        numerator = numerator / common
        denominator = denominator / common
      end
      allocate.__set__(numerator, denominator)
    end

    # A compiled literal: `3r` and `1.5r` arrive as two Integers.
    def __literal__(numerator, denominator)
      __make__(numerator, denominator)
    end

    # One argument of `Kernel#Rational`, as a Rational or a Float.
    # The tests are `Class === value` because the value may be a
    # BasicObject, which has no `is_a?` to ask.
    def __convert__(value, strict)
      return value if Rational === value || Float === value
      return __make__(value, 1) if Integer === value
      if NilClass === value
        raise TypeError, "can't convert nil into Rational"
      end
      if String === value
        parsed = __parse__(value, true)
        return parsed unless parsed.nil?
        raise ArgumentError, "invalid value for convert(): " + value.inspect
      end
      return value if Numeric === value
      unless Object === value
        # No `respond_to?` either: it is asked, and not having `to_r` is the
        # answer.
        converted = begin
          value.to_r
        rescue NoMethodError
          raise TypeError, "can't convert BasicObject into Rational"
        end
        return converted if Rational === converted
        raise TypeError, "can't convert BasicObject to Rational (BasicObject#to_r gives " + converted.class.to_s + ")"
      end
      if value.respond_to?(:to_r)
        converted = value.to_r
        return converted if Rational === converted
        raise TypeError, "can't convert " + value.class.to_s + " to Rational (" +
                         value.class.to_s + "#to_r gives " + converted.class.to_s + ")"
      end
      return __make__(Integer.__index__(value), 1) if value.respond_to?(:to_int)
      raise TypeError, "can't convert " + value.class.to_s + " into Rational"
    end

    # A String as a Rational: `sign digits [. digits] [e exponent] [/ digits]`.
    # Strict wants the whole string to be one; lenient reads what leads it and
    # answers nil only when nothing does.
    def __parse__(text, strict)
      pattern = /\A\s*([+-]?)(?:(\d+(?:_\d+)*)(?:\.(\d+(?:_\d+)*))?|\.(\d+(?:_\d+)*))(?:[eE]([+-]?\d+(?:_\d+)*))?(?:\/(\d+(?:_\d+)*))?/
      match = pattern.match(text)
      return nil if match.nil?
      return nil if strict && match.post_match !~ /\A\s*\z/
      return nil if strict && text.include?("\0")
      # `.5` has no whole part and is still a number.
      whole = match[2].nil? ? "0" : match[2].delete("_")
      fraction = (match[3] || match[4] || "").delete("_")
      numerator = (whole + fraction).to_i
      denominator = 10**fraction.size
      unless match[5].nil?
        exponent = match[5].delete("_").to_i
        if exponent >= 0
          numerator = numerator * 10**exponent
        else
          denominator = denominator * 10**-exponent
        end
      end
      unless match[6].nil?
        below = match[6].delete("_").to_i
        raise ZeroDivisionError, "divided by 0" if below == 0
        denominator = denominator * below
      end
      numerator = -numerator if match[1] == "-"
      __make__(numerator, denominator)
    end

    # The simplest fraction between two positive Rationals: the one with the
    # smallest denominator. A walk down the continued fractions of both.
    def __simplest__(low, high)
      return low if low == high
      p0 = 0
      p1 = 1
      q0 = 1
      q1 = 0
      while true
        whole = low.ceil
        break if whole < high
        k = whole - 1
        p2 = k * p1 + p0
        q2 = k * q1 + q0
        swap = Rational.__make__(1, 1) / (high - k)
        high = Rational.__make__(1, 1) / (low - k)
        low = swap
        p0 = p1
        q0 = q1
        p1 = p2
        q1 = q2
      end
      __make__(whole * p1 + p0, whole * q1 + q0)
    end
  end

  def __set__(numerator, denominator)
    @numerator = numerator
    @denominator = denominator
    freeze
  end

  def numerator = @numerator
  def denominator = @denominator

  # -- arithmetic -----------------------------------------------------------

  def +(other)
    if other.is_a?(Rational)
      Rational.__make__(@numerator * other.denominator + other.numerator * @denominator,
                        @denominator * other.denominator)
    elsif other.is_a?(Integer)
      Rational.__make__(@numerator + other * @denominator, @denominator)
    elsif other.is_a?(Float)
      to_f + other
    else
      pair = __coerce_pair__(other, true)
      pair[0] + pair[1]
    end
  end

  def -(other)
    if other.is_a?(Rational)
      Rational.__make__(@numerator * other.denominator - other.numerator * @denominator,
                        @denominator * other.denominator)
    elsif other.is_a?(Integer)
      Rational.__make__(@numerator - other * @denominator, @denominator)
    elsif other.is_a?(Float)
      to_f - other
    else
      pair = __coerce_pair__(other, true)
      pair[0] - pair[1]
    end
  end

  def *(other)
    if other.is_a?(Rational)
      Rational.__make__(@numerator * other.numerator, @denominator * other.denominator)
    elsif other.is_a?(Integer)
      Rational.__make__(@numerator * other, @denominator)
    elsif other.is_a?(Float)
      to_f * other
    else
      pair = __coerce_pair__(other, true)
      pair[0] * pair[1]
    end
  end

  def /(other)
    if other.is_a?(Rational)
      raise ZeroDivisionError, "divided by 0" if other.numerator == 0
      Rational.__make__(@numerator * other.denominator, @denominator * other.numerator)
    elsif other.is_a?(Integer)
      raise ZeroDivisionError, "divided by 0" if other == 0
      Rational.__make__(@numerator, @denominator * other)
    elsif other.is_a?(Float)
      to_f / other
    else
      pair = __coerce_pair__(other, true)
      pair[0] / pair[1]
    end
  end
  alias quo /

  def fdiv(other)
    to_f / other
  end

  # A whole exponent keeps the answer exact; any other is a Float's.
  def **(other)
    if other.is_a?(Rational) && other.denominator == 1
      other = other.numerator
    end
    if other.is_a?(Integer)
      if other >= 0
        return Rational.__make__(@numerator**other, @denominator**other)
      end
      raise ZeroDivisionError, "divided by 0" if @numerator == 0
      return Rational.__make__(@denominator**-other, @numerator**-other)
    end
    if other.is_a?(Float) || other.is_a?(Rational)
      return Rational.__make__(1, 1) if self == 1
      return to_f**other.to_f
    end
    pair = __coerce_pair__(other, true)
    pair[0]**pair[1]
  end

  def -@
    Rational.__make__(-@numerator, @denominator)
  end

  def abs
    @numerator < 0 ? -self : self
  end
  alias magnitude abs

  def div(other)
    raise ZeroDivisionError, "divided by 0" if other == 0
    (self / other).floor
  end

  def %(other)
    raise ZeroDivisionError, "divided by 0" if other == 0 && !other.is_a?(Float)
    return to_f % other if other.is_a?(Float)
    self - other * (self / other).floor
  end
  alias modulo %

  def divmod(other)
    raise ZeroDivisionError, "divided by 0" if other == 0
    quotient = (self / other).floor
    [quotient, self - other * quotient]
  end

  def remainder(other)
    self - other * (self / other).truncate
  end

  # -- comparison -----------------------------------------------------------

  def <=>(other)
    if other.is_a?(Rational)
      (@numerator * other.denominator) <=> (other.numerator * @denominator)
    elsif other.is_a?(Integer)
      @numerator <=> other * @denominator
    elsif other.is_a?(Float)
      to_f <=> other
    else
      pair = __coerce_pair__(other, false)
      pair.nil? ? nil : pair[0] <=> pair[1]
    end
  end

  def <(other) = __relop_answer__(self <=> other, other) < 0
  def <=(other) = __relop_answer__(self <=> other, other) <= 0
  def >(other) = __relop_answer__(self <=> other, other) > 0
  def >=(other) = __relop_answer__(self <=> other, other) >= 0

  def ==(other)
    if other.is_a?(Rational)
      @numerator == other.numerator && @denominator == other.denominator
    elsif other.is_a?(Integer)
      @denominator == 1 && @numerator == other
    elsif other.is_a?(Float)
      to_f == other
    else
      other == self ? true : false
    end
  end

  def eql?(other)
    other.is_a?(Rational) && self == other
  end

  def hash
    [Rational, @numerator, @denominator].hash
  end

  # An Integer joins as a Rational, a Float is joined as a Float.
  def coerce(other)
    if other.is_a?(Integer)
      [Rational.__make__(other, 1), self]
    elsif other.is_a?(Float)
      [other, to_f]
    elsif other.is_a?(Rational)
      [other, self]
    else
      raise TypeError, other.class.to_s + " can't be coerced into Rational"
    end
  end

  def zero? = @numerator == 0
  def positive? = @numerator > 0
  def negative? = @numerator < 0
  def integer? = false
  def finite? = true
  def infinite? = nil

  # -- rounding -------------------------------------------------------------

  def floor(digits = :__none__)
    __round_with__(digits) { |value| value.numerator.div(value.denominator) }
  end

  def ceil(digits = :__none__)
    __round_with__(digits) { |value| -((-value.numerator).div(value.denominator)) }
  end

  def truncate(digits = :__none__)
    __round_with__(digits) do |value|
      value.numerator < 0 ? -((-value.numerator).div(value.denominator)) : value.numerator.div(value.denominator)
    end
  end

  def to_i
    @numerator < 0 ? -((-@numerator).div(@denominator)) : @numerator.div(@denominator)
  end

  # To the nearest, a half going away from zero unless `half:` says to the
  # even neighbour or toward zero.
  def round(digits = :__none__, half: :up)
    half = :up if half.nil?
    half = half.to_sym if half.is_a?(String)
    unless half == :up || half == :even || half == :down
      raise ArgumentError, "invalid rounding mode: " + half.to_s
    end
    __round_with__(digits) do |value|
      numerator = value.numerator
      denominator = value.denominator
      negative = numerator < 0
      numerator = -numerator if negative
      whole = numerator.div(denominator)
      twice = (numerator - whole * denominator) * 2
      if twice > denominator
        whole = whole + 1
      elsif twice == denominator
        whole = whole + 1 if half == :up || (half == :even && whole.odd?)
      end
      negative ? -whole : whole
    end
  end

  # An Integer with no digits asked for, a Rational to that many otherwise.
  def __round_with__(digits)
    return yield(self) if digits == :__none__
    # An Integer and nothing that merely converts to one. Measured.
    raise TypeError, "not an integer" unless digits.is_a?(Integer)
    scale = Rational.__make__(10, 1)**digits
    scaled = Rational.__make__(yield(self * scale), 1) / scale
    digits < 1 ? scaled.to_i : scaled
  end

  # -- conversion -----------------------------------------------------------

  def to_f
    @numerator.fdiv(@denominator)
  end

  def to_r
    self
  end

  # The simplest fraction within `tolerance` of this one; itself with none.
  def rationalize(tolerance = nil)
    return self if tolerance.nil?
    tolerance = tolerance.abs
    return self if tolerance == 0
    low = self - tolerance
    high = self + tolerance
    if negative?
      -Rational.__simplest__((-high).to_r, (-low).to_r)
    else
      Rational.__simplest__(low.to_r, high.to_r)
    end
  end

  def marshal_dump
    [@numerator, @denominator]
  end
  private :marshal_dump

  def to_s
    @numerator.to_s + "/" + @denominator.to_s
  end

  def inspect
    "(" + to_s + ")"
  end
end

module Kernel
  # `Rational(a)`, `Rational(a, b)`: the fraction, from Integers, Floats,
  # Strings or other Rationals. `exception: false` answers nil where it would
  # raise.
  def Rational(first, second = 1, exception: true)
    begin
      if Integer === first && Integer === second
        raise ZeroDivisionError, "divided by 0" if second == 0
        return Rational.__make__(first, second)
      end
      first = Rational.__convert__(first, true)
      second = Rational.__convert__(second, true)
      # A Float is the fraction it exactly is.
      first = first.to_r if Float === first
      second = second.to_r if Float === second
      return first if second == 1 && Rational === first
      answer = first / second
      answer.is_a?(Integer) ? Rational.__make__(answer, 1) : answer
    rescue TypeError, ArgumentError
      raise if exception
      nil
    end
  end
end

class Integer
  def to_r = Rational.__make__(self, 1)
  def rationalize(tolerance = nil) = Rational.__make__(self, 1)
  def numerator = self
  def denominator = 1

  # `quo` is the division that does not truncate.
  def quo(other)
    return fdiv(other) if other.is_a?(Float)
    Rational.__make__(self, 1) / other
  end
end

class Float
  # Exactly: a Float is an Integer times a power of two.
  def to_r
    raise FloatDomainError, to_s if nan? || infinite?
    mantissa, exponent = Math.frexp(self)
    whole = (mantissa * 9_007_199_254_740_992).to_i
    shift = 53 - exponent
    shift > 0 ? Rational.__make__(whole, 2**shift) : Rational.__make__(whole * 2**-shift, 1)
  end

  # The simplest fraction that is still this Float — one that rounds to it —
  # or, with a tolerance, the simplest within it.
  def rationalize(tolerance = nil)
    raise FloatDomainError, to_s if nan? || infinite?
    unless tolerance.nil?
      return to_r.rationalize(tolerance)
    end
    return Rational.__make__(0, 1) if self == 0.0
    mantissa, exponent = Math.frexp(abs)
    whole = (mantissa * 9_007_199_254_740_992).to_i
    shift = 53 - exponent
    answer = if shift <= 0
               Rational.__make__(whole * 2**-shift, 1)
             else
               below = 2**(shift + 1)
               Rational.__simplest__(Rational.__make__(2 * whole - 1, below),
                                     Rational.__make__(2 * whole + 1, below))
             end
    self < 0 ? -answer : answer
  end

  def numerator
    return self if nan? || infinite?
    to_r.numerator
  end

  def denominator
    return 1 if nan? || infinite?
    to_r.denominator
  end
end

class String
  # What leads the string, read as a Rational; zero when nothing does.
  def to_r
    Rational.__parse__(self, false) || Rational.__make__(0, 1)
  end
end

class NilClass
  def to_r = Rational.__make__(0, 1)
  def rationalize(tolerance = nil) = Rational.__make__(0, 1)
end
