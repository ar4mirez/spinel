# Float.
#
# `to_s` is a primitive: printing the shortest decimal that reads back as the
# same float is an algorithm, not a formatting rule, and Ruby's answer is the
# one this has to match.
#
# Any double is a Float: one that does not fit an immediate — the infinities,
# NaN, `-0.0`, anything past about 1e77 either way — is a boxed one, and
# nothing here has to know which it holds (#18, `crates/spinel-vm/src/float.rs`).
class Float
  INFINITY = 1.0 / 0.0
  NAN = 0.0 / 0.0
  MAX = 1.7976931348623157e308
  MIN = 2.2250738585072014e-308
  EPSILON = 2.220446049250313e-16
  DIG = 15
  MANT_DIG = 53
  RADIX = 2
  MAX_EXP = 1024
  MIN_EXP = -1021
  MAX_10_EXP = 308
  MIN_10_EXP = -307

  # NaN is the one value that is not equal to itself.
  def nan?
    !(self == self)
  end

  def infinite?
    return 1 if self == INFINITY
    return -1 if self == -INFINITY
    nil
  end

  def finite?
    !nan? && infinite?.nil?
  end

  # By value, as `eql?` is: two boxed floats holding one number are one key,
  # and `0.0` and `-0.0` are `eql?`, so they hash alike. CRuby's `rb_dbl_hash`.
  def hash
    (self == 0.0 ? 0.0 : self).__bits__(64).hash
  end

  # Toward zero, as an Integer of whatever size it takes. NaN and the
  # infinities have no Integer and raise `FloatDomainError`.
  def to_i
    __math__(:truncate, self)
  end
  alias to_int to_i

  def truncate(ndigits = 0)
    ndigits = Integer.__index__(ndigits)
    return to_i if ndigits == 0
    self < 0.0 ? ceil(ndigits) : floor(ndigits)
  end

  # With no digits, an Integer. With more, a Float rounded at that decimal
  # place; with fewer, an Integer rounded at that power of ten.
  def floor(ndigits = 0)
    ndigits = Integer.__index__(ndigits)
    return __math__(:truncate, __math__(:floor, self)) if ndigits == 0
    return self if ndigits > 0 && (!finite? || ndigits >= 17)
    if ndigits > 0
      scale = 10.0**ndigits
      __math__(:floor, self * scale) / scale
    else
      to_i.floor(ndigits)
    end
  end

  def ceil(ndigits = 0)
    ndigits = Integer.__index__(ndigits)
    return __math__(:truncate, __math__(:ceil, self)) if ndigits == 0
    return self if ndigits > 0 && (!finite? || ndigits >= 17)
    if ndigits > 0
      scale = 10.0**ndigits
      __math__(:ceil, self * scale) / scale
    else
      __math__(:truncate, __math__(:ceil, self)).ceil(ndigits)
    end
  end

  # Half away from zero, unless `half:` says `:even` or `:down`.
  def round(ndigits = 0, half: :up)
    unless half.nil? || half == :up || half == :even || half == :down
      raise ArgumentError, "invalid rounding mode: #{half}"
    end
    ndigits = Integer.__index__(ndigits)
    if ndigits > 0
      # Zero keeps its sign, which arithmetic on it would lose.
      return self if !finite? || self == 0.0
      # CRuby's `float_round_overflow` and `float_round_underflow`: past the
      # digits a double holds there is nothing to round, and before the
      # first of them everything rounds away.
      exponent = __math__(:frexp, self)[1]
      return self if ndigits >= 17 - (exponent > 0 ? exponent / 4 : exponent / 3 - 1)
      return 0.0 if ndigits < -(exponent > 0 ? exponent / 3 + 1 : exponent / 4)
      scale = 10.0**ndigits
      # On the magnitude, half up first. `x * scale` can land exactly on a
      # half that `x` itself is a hair over or under, so the neighbour is
      # checked against `x` rather than trusted — CRuby's `round_half_up`.
      # The other two modes differ from it only on a true tie, which is
      # asked the same way.
      size = abs
      scaled = __math__(:round, size * scale)
      scaled += 1.0 if (scaled + 0.5) / scale <= size
      if (scaled - 0.5) / scale >= size
        tie = (scaled - 0.5) / scale == size
        if !tie || half == :down || (half == :even && scaled / 2.0 != __math__(:floor, scaled / 2.0))
          scaled -= 1.0
        end
      end
      return (self < 0.0 ? -scaled : scaled) / scale
    end
    whole = __math__(:truncate, __round__(half))
    ndigits == 0 ? whole : whole.round(ndigits)
  end

  # This value rounded to a whole number, still a Float.
  def __round__(half)
    return __math__(:round_even, self) if half == :even
    if half == :down
      return self > 0.0 ? __math__(:ceil, self - 0.5) : __math__(:floor, self + 0.5)
    end
    __math__(:round, self)
  end

  # A Float against something that is not a number is not comparable — unless
  # this is an infinity and the other says whether it is one, which is how a
  # user-defined number sits beside Infinity. CRuby's `flo_cmp`, measured.
  def <=>(other)
    return super if other.is_a?(Numeric)
    if !infinite?.nil? && other.respond_to?(:infinite?)
      sign = other.infinite?
      positive = self > 0.0
      return positive ? 1 : -1 unless sign
      order = sign <=> 0
      return positive ? (order > 0 ? 0 : 1) : (order < 0 ? 0 : -1)
    end
    super
  end

  def fdiv(other)
    self / other
  end
  alias quo fdiv

  def div(other)
    raise ZeroDivisionError, "divided by 0" if other == 0
    (self / other).floor
  end

  alias modulo %

  def divmod(other)
    raise ZeroDivisionError, "divided by 0" if other == 0
    [(self / other).floor, self % other]
  end

  def **(other)
    return Complex.__make__(self, 0)**other if Complex === other
    power = Math.__float__(other)
    # A negative number to a power that is not whole leaves the real line.
    if self < 0 && power.finite? && power != power.floor
      return Complex.__make__(self, 0)**power
    end
    __math__(:pow, self, power)
  end
  alias pow **
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

  # `-0.0.abs` is `0.0`: the sign goes even where there is nothing to negate.
  def abs
    return 0.0 if self == 0.0
    self < 0.0 ? -self : self
  end
  alias magnitude abs

  def integer?
    false
  end
end
