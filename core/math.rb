# Math — libm, with Ruby's rules about what may be asked of it (#35).
#
# One primitive, `__math__(:name, x, y)`, makes the call. What is Ruby is in
# Ruby: converting the argument the way `Float()` does, and refusing a value
# outside a function's domain with `Math::DomainError` rather than answering
# the NaN libm would. A NaN that came *in* still goes out as NaN: the domain
# checks are comparisons, and every comparison with NaN is false.
module Math
  PI = 3.141592653589793
  E = 2.718281828459045

  class DomainError < ArgumentError
  end

  # CRuby's `rb_to_float`: a Float as it is, an Integer converted, any other
  # Numeric asked for `to_f`, and nothing else — not a String, not nil.
  def self.__float__(value)
    return value if Float === value
    return value.to_f if Integer === value
    if value.nil? || true.equal?(value) || false.equal?(value)
      raise TypeError, "can't convert #{value.inspect} into Float"
    end
    raise TypeError, "can't convert #{value.class} into Float" unless Numeric === value
    converted = value.to_f
    unless Float === converted
      raise TypeError, "can't convert #{value.class} into Float (#{value.class}#to_f gives #{converted.class})"
    end
    converted
  end

  def self.__domain__(name)
    raise DomainError, "Numerical argument is out of domain - #{name}"
  end

  def sin(x) = __math__(:sin, Math.__float__(x))
  def cos(x) = __math__(:cos, Math.__float__(x))
  def tan(x) = __math__(:tan, Math.__float__(x))
  def atan(x) = __math__(:atan, Math.__float__(x))
  def sinh(x) = __math__(:sinh, Math.__float__(x))
  def cosh(x) = __math__(:cosh, Math.__float__(x))
  def tanh(x) = __math__(:tanh, Math.__float__(x))
  def asinh(x) = __math__(:asinh, Math.__float__(x))
  def exp(x) = __math__(:exp, Math.__float__(x))
  def expm1(x) = __math__(:expm1, Math.__float__(x))
  def cbrt(x) = __math__(:cbrt, Math.__float__(x))
  def erf(x) = __math__(:erf, Math.__float__(x))
  def erfc(x) = __math__(:erfc, Math.__float__(x))

  def asin(x)
    x = Math.__float__(x)
    Math.__domain__("asin") if x < -1.0 || x > 1.0
    __math__(:asin, x)
  end

  def acos(x)
    x = Math.__float__(x)
    Math.__domain__("acos") if x < -1.0 || x > 1.0
    __math__(:acos, x)
  end

  def acosh(x)
    x = Math.__float__(x)
    Math.__domain__("acosh") if x < 1.0
    __math__(:acosh, x)
  end

  def atanh(x)
    x = Math.__float__(x)
    Math.__domain__("atanh") if x < -1.0 || x > 1.0
    __math__(:atanh, x)
  end

  def atan2(y, x)
    __math__(:atan2, Math.__float__(y), Math.__float__(x))
  end

  def hypot(a, b)
    __math__(:hypot, Math.__float__(a), Math.__float__(b))
  end

  def sqrt(x)
    x = Math.__float__(x)
    Math.__domain__("sqrt") if x < 0.0
    __math__(:sqrt, x)
  end

  # The natural logarithm, or with a second argument the logarithm to that
  # base. An Integer too wide for a Float is taken apart first, so
  # `Math.log(2**2000)` is a number rather than Infinity.
  def log(x, *base)
    if base.size > 1
      raise ArgumentError, "wrong number of arguments (given #{base.size + 1}, expected 1..2)"
    end
    value = Math.__log__(x, "log")
    return value if base.empty?
    value / Math.__log__(base[0], "log")
  end

  def self.__log__(x, name)
    if Integer === x && x > 0 && x.bit_length > 1000
      # log(m * 2**e) is log(m) + e * log(2), and `m` fits.
      shift = x.bit_length - 1000
      return __math__(:log, (x >> shift).to_f) + shift * 0.6931471805599453
    end
    x = Math.__float__(x)
    Math.__domain__(name) if x < 0.0
    __math__(:log, x)
  end

  def log2(x)
    if Integer === x && x > 0 && x.bit_length > 1000
      shift = x.bit_length - 1000
      return __math__(:log2, (x >> shift).to_f) + shift
    end
    x = Math.__float__(x)
    Math.__domain__("log2") if x < 0.0
    __math__(:log2, x)
  end

  def log10(x)
    if Integer === x && x > 0 && x.bit_length > 1000
      shift = x.bit_length - 1000
      return __math__(:log10, (x >> shift).to_f) + shift * 0.3010299956639812
    end
    x = Math.__float__(x)
    Math.__domain__("log10") if x < 0.0
    __math__(:log10, x)
  end

  def log1p(x)
    x = Math.__float__(x)
    Math.__domain__("log1p") if x < -1.0
    __math__(:log1p, x)
  end

  # `[fraction, exponent]` with `x == fraction * 2**exponent`.
  def frexp(x)
    __math__(:frexp, Math.__float__(x))
  end

  def ldexp(fraction, exponent)
    fraction = Math.__float__(fraction)
    if Float === exponent
      raise RangeError, "float #{exponent} out of range of integer" unless exponent.finite?
      exponent = exponent.to_i
    end
    __math__(:ldexp, fraction, Integer.__index__(exponent))
  end

  # (n - 1)! exactly for the whole numbers a Float holds one for, and libm
  # for the rest. A negative whole number, and -Infinity, have no gamma.
  def gamma(x)
    x = Math.__float__(x)
    return x if x.nan?
    Math.__domain__("gamma") if x == -Float::INFINITY
    return x if x == Float::INFINITY
    if x == __math__(:floor, x)
      Math.__domain__("gamma") if x < 0.0
      return 1.0 / x if x == 0.0
      if x <= 23.0
        product = 1.0
        factor = 2.0
        while factor < x
          product = product * factor
          factor = factor + 1.0
        end
        return product
      end
    end
    __math__(:gamma, x)
  end

  # `[log(|gamma(x)|), sign of gamma(x)]`.
  def lgamma(x)
    x = Math.__float__(x)
    Math.__domain__("lgamma") if x == -Float::INFINITY
    return [Float::INFINITY, x.__bits__(64) >> 63 == 1 ? -1 : 1] if x == 0.0
    __math__(:lgamma, x)
  end

  module_function :sin, :cos, :tan, :asin, :acos, :atan, :atan2, :sinh, :cosh, :tanh,
                  :asinh, :acosh, :atanh, :exp, :expm1, :log, :log2, :log10, :log1p,
                  :sqrt, :cbrt, :hypot, :erf, :erfc, :gamma, :lgamma, :frexp, :ldexp
end

class Integer
  # `2 ** 0.5` is a Float, and so is anything an Integer is raised to that a
  # Float answers; a whole exponent stays the Integer primitive's.
  alias __integer_pow__ **

  def **(other)
    return to_f**other if Float === other
    __integer_pow__(other)
  end
end
