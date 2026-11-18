# frozen_string_literal: true

# `bigdecimal`'s C half on Spinel (#109). `require "bigdecimal"` is the
# bundled gem's own `bigdecimal.rb` (vendored in `stdlib/` from bigdecimal
# 4.0.1, the release Ruby 4.0 bundles), which requires this in place of
# `bigdecimal.so`; its Ruby half (`power`, `sqrt`, `BigMath.log` and `exp`,
# and `bigdecimal/util`, `math`, `jacobian`, `ludcmp`, `newton`) runs as it
# does on CRuby.
#
# What is here is `ext/bigdecimal/bigdecimal.c`, method for method, in Ruby
# over Integer: Spinel's bignum is the arbitrary-precision arithmetic, so no
# digit loop is written here. A finite, nonzero BigDecimal is a sign, an
# Integer mantissa with no trailing zeros, and the power of ten it is
# scaled by (`@__m__ * 10**@__q__`), with the mantissa's digit count kept
# beside it. Every result is the exact one, rounded once where the C rounds:
# `+`, `-` and `*` to `BigDecimal.limit`, `add`, `sub` and `mult` to the
# precision asked for, `/` and `div` to their quotient's digit count, each in
# the thread's rounding mode, as VpMidRound rounds.
#
# bigdecimal.c keeps its digits in base 10**9 words, and a few things a
# program can see are counted in words rather than digits; they are counted
# the same way here, from where the digits fall against the word boundaries:
# the precision `_dump` writes (`Prec * BASE_FIG`), the digits a Rational is
# converted to when it meets a BigDecimal (`GetCoercePrec`), and how far to
# the left `round` can go before it answers zero in a mode other than
# ceiling and floor. A NaN or Infinity that `/` answers is dumped with the
# word count VpDivd left in it ("45:Infinity").
#
# ponytail: a sum is computed exactly before it is rounded, so adding
# operands whose exponents are far apart builds every digit between them as
# one Integer and counts them through `Integer#to_s`, which is superlinear:
# 1e1000000 + 1 takes a tenth of a second where bigdecimal.c takes a
# millisecond, and 1e1000000000 + 1 does not finish in a minute where the C
# answers in about one. Upgrade: when a limit or precision will round the
# sum, compute only the digits it keeps and whether any below are nonzero.
#
# The exception mode, rounding mode and precision limit are the current
# thread's, kept where bigdecimal.c keeps them: `Thread#[]` under
# `:"BigDecimal.exception_mode"`, `:"BigDecimal.rounding_mode"` and
# `:"BigDecimal.precision_limit"`, set to the default when first read.

class BigDecimal < Numeric
  # The parts of a value, read by the methods below on the other operand.
  # `@__s__` is the sign as `BigDecimal#sign` answers it (0 NaN, 1 and -1
  # zero, 2 and -2 finite, 3 and -3 infinite).
  attr_reader :__s__, :__m__, :__q__, :__n__

  class << self
    alias_method :__alloc__, :allocate
    private :__alloc__

    # `rb_undef_alloc_func`.
    def allocate
      raise TypeError, "allocator undefined for #{self}"
    end

    undef_method :new
  end

  # VpAlloc's result, frozen, as every BigDecimal is. `words` is the word
  # count a NaN, infinity or zero from VpDivd keeps; `stale` the word
  # exponent a zero kept from what it was parsed or rounded from.
  def __init__(sign, mantissa, scale, digits, words = nil, stale = nil)
    @__s__ = sign
    @__m__ = mantissa
    @__q__ = scale
    @__n__ = digits
    @__p__ = words
    @__e__ = stale
    freeze
  end
  private :__init__

  VERSION = "4.0.1"
  BASE = 1_000_000_000

  EXCEPTION_ALL = 0xFF
  EXCEPTION_NaN = 0x02
  EXCEPTION_INFINITY = 0x01
  EXCEPTION_UNDERFLOW = 0x04
  EXCEPTION_OVERFLOW = 0x01
  EXCEPTION_ZERODIVIDE = 0x10

  ROUND_MODE = 0x100
  ROUND_UP = 1
  ROUND_DOWN = 2
  ROUND_HALF_UP = 3
  ROUND_HALF_DOWN = 4
  ROUND_CEILING = 5
  ROUND_FLOOR = 6
  ROUND_HALF_EVEN = 7

  SIGN_NaN = 0
  SIGN_POSITIVE_ZERO = 1
  SIGN_NEGATIVE_ZERO = -1
  SIGN_POSITIVE_FINITE = 2
  SIGN_NEGATIVE_FINITE = -2
  SIGN_POSITIVE_INFINITE = 3
  SIGN_NEGATIVE_INFINITE = -3

  # The VP routines: what bigdecimal.c does to a Real, said over
  # (sign, mantissa, scale).
  module Vp__
    BASE_FIG = 9
    DOUBLE_FIG = 16
    # VP_EXPONENT_MAX and VP_EXPONENT_MIN, in words, on a 64-bit build.
    EXPONENT_MAX = 9_223_372_036_854_775_807 / 9
    EXPONENT_MIN = -EXPONENT_MAX
    # A scale this far from zero may be near an exponent limit, so the word
    # exponent is checked; nearer, no step can reach one.
    FAR = 4_000_000_000_000_000_000

    VP_EXCEPTION_OP = 0x20

    EXCEPTION_KEY = :"BigDecimal.exception_mode"
    ROUNDING_KEY = :"BigDecimal.rounding_mode"
    LIMIT_KEY = :"BigDecimal.precision_limit"

    ROUNDING_MODES = {
      up: 1, down: 2, half_up: 3, half_down: 4, ceil: 5, floor: 6,
      half_even: 7, default: 3, truncate: 2, banker: 7, ceiling: 5
    }.freeze

    # --- the thread's modes ---------------------------------------------

    def self.exception_mode
      mode = Thread.current[EXCEPTION_KEY]
      return mode unless mode.nil?
      Thread.current[EXCEPTION_KEY] = 0
      0
    end

    def self.rounding_mode
      mode = Thread.current[ROUNDING_KEY]
      return mode unless mode.nil?
      Thread.current[ROUNDING_KEY] = BigDecimal::ROUND_HALF_UP
      BigDecimal::ROUND_HALF_UP
    end

    def self.limit
      limit = Thread.current[LIMIT_KEY]
      return limit unless limit.nil?
      Thread.current[LIMIT_KEY] = 0
      0
    end

    def self.limit=(limit)
      Thread.current[LIMIT_KEY] = limit
    end

    # VpException: a FloatDomainError when the mode asks for one.
    def self.exception(flag, message, always = false)
      always = true if flag == VP_EXCEPTION_OP
      if always || exception_mode & flag != 0
        raise FloatDomainError, message
      end
      nil
    end

    # VpCheckException.
    def self.check(value, always = false)
      case value.__s__
      when 0
        exception(BigDecimal::EXCEPTION_NaN, "Computation results in 'NaN' (Not a Number)", always)
      when 3
        exception(BigDecimal::EXCEPTION_INFINITY, "Computation results in 'Infinity'", always)
      when -3
        exception(BigDecimal::EXCEPTION_INFINITY, "Computation results in '-Infinity'", always)
      end
      value
    end

    # --- making values --------------------------------------------------

    def self.make(sign, mantissa = 0, scale = 0, digits = 0, words = nil, klass = BigDecimal, stale = nil)
      value = klass.__send__(:__alloc__)
      value.__send__(:__init__, sign, mantissa, scale, digits, words, stale)
      value
    end

    # A zero whose Real kept the word exponent of what it was parsed or
    # rounded from: VpSetZero leaves `exponent` alone, and BigDecimal_to_i
    # reads it, answering (0/1) for one of 2 or more.
    def self.stale_zero(sign, word, klass = BigDecimal)
      make(sign < 0 ? -1 : 1, 0, 0, 0, nil, klass, word >= 2 ? word : nil)
    end

    def self.nan(words = nil) = make(0, 0, 0, 0, words)
    def self.infinity(sign, words = nil) = make(sign < 0 ? -3 : 3, 0, 0, 0, words)
    def self.zero(sign, words = nil) = make(sign < 0 ? -1 : 1, 0, 0, 0, words)

    # The words an exponent of ten falls in: `exponent` of a Real whose
    # decimal exponent (BigDecimal#exponent) is `e`.
    def self.word_exponent(e) = (e + 8).div(9)

    # A finite value from an exact sign, mantissa and scale, its trailing
    # zeros taken off. VpNmlz's AddExponent: a value below the smallest
    # exponent is zero (`Exponent underflow`), above the largest infinite
    # (`Exponent overflow`).
    def self.finite(sign, mantissa, scale, klass = BigDecimal, check_overflow = true)
      return zero(sign) if mantissa == 0
      digits = mantissa.to_s
      size = digits.bytesize
      if digits.getbyte(size - 1) == 48
        kept = size - 1
        kept -= 1 while digits.getbyte(kept - 1) == 48
        mantissa = digits.byteslice(0, kept).to_i
        scale += size - kept
        size = kept
      end
      if scale > FAR || scale < -FAR
        word = word_exponent(scale + size)
        if word < EXPONENT_MIN
          exception(BigDecimal::EXCEPTION_UNDERFLOW, "Exponent underflow")
          return zero(sign)
        end
        if check_overflow && word > EXPONENT_MAX
          exception(BigDecimal::EXCEPTION_OVERFLOW, "Exponent overflow")
          return infinity(sign)
        end
      end
      make(sign < 0 ? -2 : 2, mantissa, scale, size, nil, klass)
    end

    # AddExponent's overflow and underflow on a word exponent a step makes
    # before it normalizes: nil when it is in range, else the result.
    def self.exponent_range(word, sign, words = nil)
      if word > EXPONENT_MAX
        exception(BigDecimal::EXCEPTION_OVERFLOW, "Exponent overflow")
        infinity(sign, words)
      elsif word < EXPONENT_MIN
        exception(BigDecimal::EXCEPTION_UNDERFLOW, "Exponent underflow")
        zero(sign, words)
      end
    end

    # A count as bigdecimal.c's `ssize_t` holds it: past 2**63 it wraps.
    def self.ssize(value)
      return value if value <= 9_223_372_036_854_775_807 && value >= -9_223_372_036_854_775_808
      ((value + 9_223_372_036_854_775_808) % 18_446_744_073_709_551_616) - 9_223_372_036_854_775_808
    end

    # --- rounding ---------------------------------------------------------

    # VpMidRound: `value` rounded to `places` digits after the decimal
    # point (to the left of it when negative) in `mode`, for a finite,
    # nonzero value. When the place is left of the value's leading word
    # altogether, every mode but ceiling and floor answers zero, as the C
    # does, ROUND_UP included.
    def self.round_places(value, places, mode)
      sign = value.__s__ > 0 ? 1 : -1
      mantissa = value.__m__
      scale = value.__q__
      word = word_exponent(scale + value.__n__)
      shifted = places + 9 * word
      if shifted < 0
        unless mode == BigDecimal::ROUND_CEILING || mode == BigDecimal::ROUND_FLOOR
          return stale_zero(sign, word, value.class)
        end
        # VpMidRound's exptoadd, which moves even a zero's exponent.
        word += -shifted / 9
      end
      drop = -places - scale
      return value if drop <= 0
      if drop > value.__n__
        quotient = 0
        half = -1
      else
        divisor = 10**drop
        quotient, rest = mantissa.divmod(divisor)
        half = (rest * 2) <=> divisor
      end
      # The remainder is never zero: the mantissa has no trailing zeros.
      up = case mode
           when BigDecimal::ROUND_DOWN then false
           when BigDecimal::ROUND_UP then true
           when BigDecimal::ROUND_HALF_UP then half >= 0
           when BigDecimal::ROUND_HALF_DOWN then half > 0
           when BigDecimal::ROUND_CEILING then sign > 0
           when BigDecimal::ROUND_FLOOR then sign < 0
           when BigDecimal::ROUND_HALF_EVEN then half > 0 || (half == 0 && quotient.odd?)
           else false
           end
      quotient += 1 if up
      return stale_zero(sign, word, value.class) if quotient == 0
      finite(sign, quotient, -places, value.class, false)
    end

    # VpLeftRound: `value` rounded to `digits` significant digits.
    def self.round_digits(value, digits, mode)
      return value unless value.__s__.abs == 2 && value.__n__ > digits
      round_places(value, digits - (value.__q__ + value.__n__), mode)
    end

    # VpLimitRound: rounded to the thread's precision limit, if it has one.
    def self.limit_round(value, limit = self.limit)
      return value if limit == 0
      round_digits(value, limit, rounding_mode)
    end

    # --- what a Real's words say ------------------------------------------

    # `Prec`: how many base 10**9 words the digits take.
    def self.words(value)
      if value.__s__.abs == 2
        word = word_exponent(value.__q__ + value.__n__)
        leading = 9 * word - (value.__q__ + value.__n__)
        (leading + value.__n__ + 8) / 9
      else
        value.instance_variable_get(:@__p__) || 1
      end
    end

    # GetCoercePrec.
    def self.coerce_precision(value, precision)
      precision = words(value) * BASE_FIG if precision == 0
      precision < 2 * DOUBLE_FIG ? 2 * DOUBLE_FIG : precision
    end

    # --- conversions --------------------------------------------------------

    # GetBDValueWithPrec: a BigDecimal for an operand, or nil for one the
    # caller must coerce. `must` raises for those instead. A Rational is
    # divided out to GetCoercePrec(receiver, precision) digits.
    def self.operand(value, receiver, precision, must)
      if value.is_a?(BigDecimal)
        value
      elsif value.is_a?(Integer)
        from_integer(value)
      elsif value.is_a?(Float)
        from_float(value, 0, true)
      elsif value.is_a?(Rational)
        from_rational(value, coerce_precision(receiver, precision), true)
      else
        cannot_coerce(value) if must
        nil
      end
    end

    # cannot_be_coerced_into_BigDecimal.
    def self.cannot_coerce(value, klass = TypeError)
      name = if value.nil? || true.equal?(value) || false.equal?(value) ||
                value.is_a?(Symbol) || value.is_a?(Integer) || value.is_a?(Float)
               value.inspect
             else
               value.class.to_s
             end
      raise klass, "#{name} can't be coerced into BigDecimal"
    end

    def self.from_integer(integer)
      return zero(1) if integer == 0
      finite(integer < 0 ? -1 : 1, integer.abs, 0)
    end

    # rb_float_convert_to_BigDecimal: the digits of `float`'s shortest
    # representation, at most 16 of them (cut, not rounded), when `digits`
    # is 0, else its exact value rounded half to even to `digits`
    # significant digits, as dtoa's mode 2 is.
    def self.from_float(float, digits, raise_exception)
      if float.nan?
        return check(nan)
      elsif float.infinite?
        return check(infinity(float))
      elsif float == 0.0
        return zero(1.0 / float)
      end
      if digits > DOUBLE_FIG
        return nil unless raise_exception
        raise ArgumentError, "precision too large."
      end
      sign = float < 0 ? -1 : 1
      if digits == 0
        text = float.abs.to_s
        mantissa_text, exponent_text = text.split("e")
        exponent = exponent_text ? exponent_text.to_i : 0
        whole, fraction = mantissa_text.split(".")
        fraction = "" if fraction == "0"
        all = whole + fraction
        lead = 0
        lead += 1 while all.getbyte(lead) == 48
        point = whole.bytesize - lead + exponent
        all = all.byteslice(lead, all.bytesize - lead)
        all = all.byteslice(0, DOUBLE_FIG) if all.bytesize > DOUBLE_FIG
        finite(sign, all.to_i, point - all.bytesize)
      else
        digits_list, k = dtoa_mode2(float.abs, digits)
        finite(sign, digits_list.join.to_i, k + 1 - digits_list.size)
      end
    end

    # BigDecimal_dtoa's mode 2 (missing/dtoa.c, Ruby's copy of David Gay's
    # dtoa): `digits` significant digits of a positive, finite double, and
    # the decimal exponent of the first (value = 0.d1d2... * 10**(k + 1)).
    #
    # Ported step for step, in the same double arithmetic, because its
    # answer is not always the double's exact value rounded: under 15
    # digits it first tries floating point, and when what is left after the
    # last digit is within its error bound of one half it rounds up if that
    # digit is odd (BigDecimal(2.675, 3) is 0.268e1, though the double is
    # 2.67499999999999982236...), and otherwise remembers the tie (`half`),
    # which the exact path then honours by rounding up only an odd digit.
    TENS = (0..22).map { |i| "1e#{i}".to_f }.freeze
    BIGTENS = [1e16, 1e32, 1e64, 1e128, 1e256].freeze

    def self.dtoa_mode2(d, ndigits)
      fraction, binary = Math.frexp(d)
      if d < 2.2250738585072014e-308
        # A subnormal's estimate reads the top 32 bits of its significand.
        d2 = (fraction * 4_294_967_296.0).floor / 2_147_483_648.0
      else
        d2 = fraction * 2.0
      end
      i = binary - 1
      ds = (d2 - 1.5) * 0.289529654602168 + 0.1760912590558 + i * 0.301029995663981
      k = ds.to_i
      k -= 1 if ds < 0.0 && ds != k
      k_check = true
      if k >= 0 && k <= 22
        k -= 1 if d < TENS[k]
        k_check = false
      end
      ilim = ndigits
      half = false
      if ilim <= 14
        # Try to get by with floating-point arithmetic.
        dd = d
        ieps = 2
        kq = k
        il = ilim
        n = 0
        if kq > 0
          ds = TENS[kq & 0xf]
          j = kq >> 4
          if j & 0x10 != 0
            j &= 0xf
            dd /= BIGTENS[4]
            ieps += 1
          end
          while j != 0
            if j & 1 != 0
              ieps += 1
              ds *= BIGTENS[n]
            end
            j >>= 1
            n += 1
          end
          dd /= ds
        elsif kq < 0
          j1 = -kq
          dd *= TENS[j1 & 0xf]
          j = j1 >> 4
          while j != 0
            if j & 1 != 0
              ieps += 1
              dd *= BIGTENS[n]
            end
            j >>= 1
            n += 1
          end
        end
        if k_check && dd < 1.0 && il > 0
          kq -= 1
          dd *= 10.0
          ieps += 1
        end
        eps = (ieps * dd + 7.0) * 2.220446049250313e-16
        eps *= TENS[il - 1]
        out = []
        n = 1
        loop do
          l = dd.to_i
          dd -= l
          il = n if dd == 0.0
          out << l
          if n == il
            return dtoa_bump(out, kq) if dd > 0.5 + eps
            if dd < 0.5 - eps
              out.pop while out.last == 0
              return [out, kq]
            end
            half = true
            return dtoa_bump(out, kq) if l.odd?
            break
          end
          n += 1
          dd *= 10.0
        end
      end
      # A "small" integer.
      if k <= 14 && d == d.floor
        ds = TENS[k]
        dd = d
        out = []
        n = 1
        loop do
          l = (dd / ds).to_i
          dd -= l * ds
          out << l
          break if dd == 0.0
          if n == ilim
            dd += dd
            return dtoa_bump(out, k) if dd > ds || (dd == ds && l.odd?)
            break
          end
          n += 1
          dd *= 10.0
        end
        return [out, k]
      end
      # The exact digits.
      exact = d.to_r
      numerator = exact.numerator
      denominator = exact.denominator
      k += 1 until __below__(numerator, denominator, k + 1)
      k -= 1 while __below__(numerator, denominator, k)
      shift = ilim - 1 - k
      if shift >= 0
        divisor = denominator
        whole, rest = (numerator * 10**shift).divmod(divisor)
      else
        divisor = denominator * 10**-shift
        whole, rest = numerator.divmod(divisor)
      end
      out = whole.digits.reverse
      if rest == 0
        out.pop while out.last == 0
        return [out, k]
      end
      twice = rest * 2 <=> divisor
      unless twice > 0 || (twice == 0 && out.last.odd?)
        out.pop while out.last == 0
        return [out, k]
      end
      # roundoff, where a remembered tie rounds up only an odd digit.
      index = out.size - 1
      while out[index] == 9
        return [[1], k + 1] if index == 0
        index -= 1
      end
      out[index] += 1 if !half || out[index].odd?
      [out[0..index], k]
    end

    # dtoa's bump_up: one added to the last digit, carried.
    def self.dtoa_bump(out, k)
      index = out.size - 1
      while out[index] == 9
        if index == 0
          k += 1
          out[0] = 0
          break
        end
        index -= 1
      end
      out[index] += 1
      [out[0..index], k]
    end

    # Whether numerator/denominator < 10**point.
    def self.__below__(numerator, denominator, point)
      if point >= 0
        numerator < denominator * 10**point
      else
        numerator * 10**-point < denominator
      end
    end

    # rb_rational_convert_to_BigDecimal.
    def self.from_rational(rational, digits, raise_exception)
      if digits.nil?
        return nil unless raise_exception
        raise ArgumentError, "can't omit precision for a #{rational.class}."
      end
      from_integer(rational.numerator).__send__(:__div2__, rational.denominator, digits)
    end

    # rb_convert_to_BigDecimal, BigDecimal()'s conversion.
    def self.convert(value, digits, raise_exception)
      if value.nil? || true.equal?(value) || false.equal?(value)
        return nil unless raise_exception
        raise TypeError, "can't convert #{value.inspect} into BigDecimal"
      end
      case value
      when BigDecimal
        return check(value) if digits.nil?
        copy = make(value.__s__, value.__m__, value.__q__, value.__n__, value.instance_variable_get(:@__p__),
                    BigDecimal, value.instance_variable_get(:@__e__))
        check(copy)
      when Integer
        from_integer(value)
      when Float
        from_float(value, digits || 0, raise_exception)
      when Rational
        from_rational(value, digits, raise_exception)
      when Complex
        imaginary = value.imaginary
        zero = case imaginary
               when Integer then imaginary == 0
               when Rational then imaginary.numerator == 0
               else imaginary == 0
               end
        unless zero
          raise ArgumentError, "Unable to make a BigDecimal from non-zero imaginary number"
        end
        convert(value.real, digits, raise_exception)
      when String
        parse(value, BigDecimal, true, raise_exception)
      else
        string = String.try_convert(value)
        unless string.is_a?(String)
          return nil unless raise_exception
          raise TypeError, "can't convert #{value.class} into BigDecimal"
        end
        parse(string, BigDecimal, true, raise_exception)
      end
    end

    # --- VpAlloc and VpCtoV: a String's value ----------------------------

    PLAIN = /\A[ \t\n\v\f\r]*([-+]?)([0-9]*)(?:\.([0-9]*))?(?:[eEdD]([-+]?)([0-9]+))?[ \t\n\v\f\r]*\z/n
    SPECIAL = [["Infinity", 3], ["+Infinity", 3], ["-Infinity", -3], ["NaN", 0]].freeze

    def self.space?(byte) = byte == 32 || (byte && byte >= 9 && byte <= 13)
    def self.digit?(byte) = byte && byte >= 48 && byte <= 57

    # The value of `string` as BigDecimal() reads it (`strict`) or
    # BigDecimal.interpret_loosely does; nil when it is not a number and
    # `raise_exception` is false.
    def self.parse(string, klass, strict, raise_exception)
      if string.include?("\0")
        raise ArgumentError, "string contains null byte"
      end
      bytes = string.b
      # The common case, a plain number, read at once; anything else (an
      # underscore, `#`, NaN or Infinity, garbage) goes the long way, which
      # is VpAlloc byte for byte.
      if strict && (plain = PLAIN.match(bytes)) && (!plain[2].empty? || (plain[3] && !plain[3].empty?))
        return build(plain[1] == "-" ? -1 : 1, plain[2], plain[3] || "", plain[5] || "",
                     plain[4] == "-" ? -1 : 1, klass)
      end
      size = bytes.bytesize
      j = 0
      j += 1 while j < size && space?(bytes.getbyte(j))
      SPECIAL.each do |text, sign|
        next unless bytes.byteslice(j, text.bytesize) == text
        rest = j + text.bytesize
        rest += 1 while rest < size && space?(bytes.getbyte(rest))
        next unless rest == size
        return sign == 0 ? check(nan_for(klass)) : check(make(sign, 0, 0, 0, nil, klass))
      end
      j += 1 if bytes.getbyte(j) == 35 # '#'
      sign = 1
      byte = bytes.getbyte(j)
      if byte == 45 || byte == 43
        sign = -1 if byte == 45
        j += 1
      end
      integer = +""
      fraction = +""
      exponent = +""
      exponent_sign = 1
      exp_seen = false
      invalid = false
      # The integer part.
      v = nil
      loop do
        v = bytes.getbyte(j)
        break if v.nil?
        if !strict && space?(v)
          v = nil
          break
        end
        if v == 95 # '_'
          if integer.bytesize > 0
            after = bytes.getbyte(j + 1)
            if after.nil? || space?(after) || digit?(after)
              j += 1
              next
            end
            unless strict
              v = nil
              break
            end
          end
          invalid = true
          break
        end
        break unless digit?(v)
        integer << v
        j += 1
      end
      if !invalid && !v.nil?
        if v == 46 # '.'
          j += 1
          loop do
            v = bytes.getbyte(j)
            break if v.nil?
            if !strict && space?(v)
              v = nil
              break
            end
            if v == 95
              if fraction.bytesize > 0 && digit?(bytes.getbyte(j + 1))
                j += 1
                next
              end
              unless strict
                v = nil
                break
              end
              invalid = true
              break
            end
            break unless digit?(v)
            fraction << v
            j += 1
          end
        end
        if !invalid && !v.nil?
          case v
          when 101, 69, 100, 68 # e E d D
            exp_seen = true
            j += 1
            v = bytes.getbyte(j)
            if v == 45 || v == 43
              exponent_sign = -1 if v == 45
              j += 1
            end
            loop do
              v = bytes.getbyte(j)
              break if v.nil?
              if !strict && space?(v)
                v = nil
                break
              end
              if v == 95
                if exponent.bytesize > 0 && digit?(bytes.getbyte(j + 1))
                  j += 1
                  next
                end
                unless strict
                  v = nil
                  exp_seen = false if exponent.bytesize == 0
                  break
                end
                invalid = true
                break
              end
              break unless digit?(v)
              exponent << v
              j += 1
            end
          end
        end
        if !invalid && !v.nil?
          j += 1 while j < size && space?(bytes.getbyte(j))
          invalid = true if j < size && strict
        end
      end
      if !invalid && strict &&
         ((integer.empty? && fraction.empty?) || (exp_seen && exponent.empty?))
        invalid = true
      end
      if invalid
        return zero(1) unless strict
        return nil unless raise_exception
        # The string as it came, between quotes, in bytes, as rb_raise
        # formats it.
        raise ArgumentError, "invalid value for BigDecimal(): \"".b + string.b + "\"".b
      end
      build(sign, integer, fraction, exponent, exponent_sign, klass)
    end

    # VpCtoV: the value of a String's sign, digits and exponent.
    def self.build(sign, integer, fraction, exponent, exponent_sign, klass)
      e = 0
      overflow = false
      exponent.each_byte do |digit|
        tens = e * 10
        next_e = tens + exponent_sign * (digit - 48)
        if tens > 9_223_372_036_854_775_807 || tens < -9_223_372_036_854_775_808 ||
           next_e > 9_223_372_036_854_775_807 || next_e < -9_223_372_036_854_775_808
          overflow = true
          break
        end
        e = next_e
      end
      lead = 0
      lead += 1 while integer.getbyte(lead) == 48
      significant = integer.bytesize - lead
      position = e + significant
      if position > 9_223_372_036_854_775_807
        overflow = true
      else
        e = position
      end
      if overflow || word_exponent(e) > EXPONENT_MAX || word_exponent(e) < EXPONENT_MIN
        digits = integer + fraction
        nonzero = digits.each_byte.any? { |d| d != 48 }
        if nonzero && e > 0
          value = make(sign < 0 ? -3 : 3, 0, 0, 0, nil, klass)
          exception(BigDecimal::EXCEPTION_INFINITY, "exponent overflow")
          return check(value)
        end
        return make(sign < 0 ? -1 : 1, 0, 0, 0, nil, klass)
      end
      mantissa = (integer + fraction).to_i
      return check(stale_zero(sign, word_exponent(e), klass)) if mantissa == 0
      check(finite(sign, mantissa, e - significant - fraction.bytesize, klass))
    end

    def self.nan_for(klass) = make(0, 0, 0, 0, nil, klass)

    # --- the operators ------------------------------------------------------

    # VpIsDefOP's answer when an operand is NaN or infinite, after the
    # exception it raises when the mode asks.
    def self.special_result(sign)
      if sign == 0
        exception(BigDecimal::EXCEPTION_NaN, "Computation results to 'NaN'")
        nan
      elsif sign > 0
        exception(BigDecimal::EXCEPTION_INFINITY, "Computation results to 'Infinity'")
        infinity(1)
      else
        exception(BigDecimal::EXCEPTION_INFINITY, "Computation results to '-Infinity'")
        infinity(-1)
      end
    end

    # VpAddSub: `a + operation * b` for two operands neither of them NaN,
    # rounded to `limit` digits.
    def self.add(a, b, operation, limit)
      as = a.__s__
      bs = b.__s__ * operation
      if as.abs == 3 || bs.abs == 3
        if as.abs == 3 && bs.abs == 3
          return special_result(as == bs ? as : 0)
        end
        return special_result(as.abs == 3 ? as : bs)
      end
      if as.abs == 1
        if bs.abs == 1
          return zero(as < 0 && bs < 0 ? -1 : 1)
        end
        return limit_round(make(bs, b.__m__, b.__q__, b.__n__), limit)
      end
      return limit_round(a, limit) if bs.abs == 1
      aq = a.__q__
      bq = b.__q__
      if aq > FAR || bq > FAR
        # VpSetPTR's AddExponent(c, 1) on the larger operand's exponent.
        word_a = word_exponent(aq + a.__n__)
        word_b = word_exponent(bq + b.__n__)
        if (word_a > word_b ? word_a : word_b) + 1 > EXPONENT_MAX
          exception(BigDecimal::EXCEPTION_OVERFLOW, "Exponent overflow")
          larger = word_a >= word_b ? as : bs
          return infinity((as < 0) == (bs < 0) ? as : larger)
        end
      end
      low = aq < bq ? aq : bq
      x = a.__m__ * 10**(aq - low)
      x = -x if as < 0
      y = b.__m__ * 10**(bq - low)
      y = -y if bs < 0
      sum = x + y
      return zero(1) if sum == 0
      limit_round(finite(sum < 0 ? -1 : 1, sum.abs, low), limit)
    end

    # VpMult: the exact product of two operands.
    def self.multiply(a, b)
      as = a.__s__
      bs = b.__s__
      sign = (as < 0) == (bs < 0) ? 1 : -1
      if as == 0 || bs == 0
        return special_result(0)
      end
      if as.abs == 3 || bs.abs == 3
        return special_result(0) if as.abs == 1 || bs.abs == 1
        return special_result(sign)
      end
      return zero(sign) if as.abs == 1 || bs.abs == 1
      aq = a.__q__
      bq = b.__q__
      if aq > FAR || aq < -FAR || bq > FAR || bq < -FAR
        # AddExponent(c, b->exponent), unless VpIsOne took the other.
        unless a.__m__ == 1 && aq + a.__n__ == 1 || b.__m__ == 1 && bq + b.__n__ == 1
          out = exponent_range(word_exponent(aq + a.__n__) + word_exponent(bq + b.__n__), sign)
          return out if out
        end
      end
      finite(sign, a.__m__ * b.__m__, aq + bq)
    end

    # VpDivd with BigDecimal_div2's rounding: a / b to `digits` significant
    # digits in the rounding mode. A NaN or infinite quotient keeps the
    # word count the C leaves in it.
    def self.divide(a, b, digits)
      as = a.__s__
      bs = b.__s__
      words = (digits + 8) / 9 + 1
      sign = (as < 0) == (bs < 0) ? 1 : -1
      if as == 0 || bs == 0 || (as.abs == 3 && bs.abs == 3)
        exception(BigDecimal::EXCEPTION_NaN, "Computation results to 'NaN'")
        return nan(words)
      end
      if as.abs == 3
        special_result(sign)
        return infinity(sign, words)
      end
      if bs.abs == 3
        # VpIsDefOP sets a zero, then words it as an infinity.
        exception(BigDecimal::EXCEPTION_INFINITY, "Computation results to '-Infinity'")
        return zero(sign, words)
      end
      if as.abs == 1 && bs.abs == 1
        exception(BigDecimal::EXCEPTION_NaN, "Computation results to 'NaN'")
        return nan(words)
      end
      if bs.abs == 1
        exception(BigDecimal::EXCEPTION_ZERODIVIDE, "Divide by zero")
        return infinity(sign, words)
      end
      return zero(sign) if as.abs == 1
      if a.__q__ > FAR || a.__q__ < -FAR || b.__q__ > FAR || b.__q__ < -FAR
        word_a = word_exponent(a.__q__ + a.__n__)
        word_b = word_exponent(b.__q__ + b.__n__)
        out = exponent_range(word_a + 1, sign, words) || exponent_range(word_a + 1 - word_b, sign, words)
        return out if out
      end
      shift = digits + 2 + b.__n__ - a.__n__
      if shift >= 0
        quotient, rest = (a.__m__ * 10**shift).divmod(b.__m__)
      else
        quotient, rest = a.__m__.divmod(b.__m__ * 10**-shift)
      end
      scale = a.__q__ - b.__q__ - shift
      if rest != 0
        # A digit past the last that keeps the quotient from looking exact.
        quotient = quotient * 10 + 1
        scale -= 1
      end
      round_digits(finite(sign, quotient, scale, BigDecimal, false), digits, rounding_mode)
    end

    # BigDecimal_DoDivmod for two finite operands, `b` nonzero: the
    # quotient as an Integer, floored or truncated, and the remainder.
    def self.divmod(a, b, truncate)
      as = a.__s__
      bs = b.__s__
      t = a.__q__ - b.__q__
      if t >= 0
        quotient = (a.__m__ * 10**t) / b.__m__
      else
        quotient = a.__m__ / (b.__m__ * 10**-t)
      end
      low = a.__q__ < b.__q__ ? a.__q__ : b.__q__
      x = a.__m__ * 10**(a.__q__ - low)
      y = b.__m__ * 10**(b.__q__ - low)
      rest = x - quotient * y
      negative = (as < 0) != (bs < 0)
      if rest == 0
        return [negative ? -quotient : quotient, zero(1)]
      end
      if !truncate && negative
        [-(quotient + 1), finite(bs < 0 ? -1 : 1, y - rest, low)]
      else
        [negative ? -quotient : quotient, finite(as < 0 ? -1 : 1, rest, low)]
      end
    end

    # VpComp: -1, 0, 1, or nil for NaN.
    def self.compare(a, b)
      as = a.__s__
      bs = b.__s__
      return nil if as == 0 || bs == 0
      if as.abs == 3 || bs.abs == 3
        return as.abs == 3 && bs.abs == 3 ? (as <=> bs) : (as.abs == 3 ? (as <=> 0) : (0 <=> bs))
      end
      return 0 if as.abs == 1 && bs.abs == 1
      return (as <=> 0) if bs.abs == 1
      return (0 <=> bs) if as.abs == 1
      return as <=> bs if (as < 0) != (bs < 0)
      ea = a.__q__ + a.__n__
      eb = b.__q__ + b.__n__
      result = if ea != eb
                 ea <=> eb
               else
                 low = a.__q__ < b.__q__ ? a.__q__ : b.__q__
                 (a.__m__ * 10**(a.__q__ - low)) <=> (b.__m__ * 10**(b.__q__ - low))
               end
      as < 0 ? -result : result
    end

    # check_rounding_mode.
    def self.rounding_mode_of(value)
      if value.is_a?(Symbol)
        mode = ROUNDING_MODES[value]
        raise ArgumentError, "invalid rounding mode (#{value})" if mode.nil?
        return mode
      end
      mode = Integer.__c_long__(value)
      if mode < 0 || mode > 65_535
        raise RangeError, "integer #{mode} too #{mode < 0 ? 'small' : 'big'} to convert to 'unsigned short'"
      end
      raise ArgumentError, "invalid rounding mode (#{value})" unless mode >= 1 && mode <= 7
      mode
    end

    # check_rounding_mode_option: the `half:` of a Hash.
    def self.rounding_option(options)
      mode = options.fetch(:half, nil)
      return rounding_mode if mode.nil?
      text = if mode.is_a?(Symbol)
               mode.to_s
             elsif mode.is_a?(String)
               mode
             else
               String.try_convert(mode)
             end
      if text
        case text.downcase
        when "up" then return BigDecimal::ROUND_HALF_UP
        when "even" then return BigDecimal::ROUND_HALF_EVEN
        when "down" then return BigDecimal::ROUND_HALF_DOWN
        end
        mode = text
      end
      raise ArgumentError, "invalid rounding mode (#{mode})"
    end

    # NUM2INT.
    def self.int(value)
      value = Integer.__c_long__(value)
      if value > 2_147_483_647 || value < -2_147_483_648
        raise RangeError, "integer #{value} too #{value < 0 ? 'small' : 'big'} to convert to 'int'"
      end
      value
    end

    # check_int_precision.
    def self.precision(value)
      value = Integer.__c_long__(value)
      raise ArgumentError, "negative precision" if value < 0
      value
    end

    # --- VpToString and VpToFString -----------------------------------------

    # The digits of a finite value with its sign, `fPlus` and grouping.
    def self.sign_prefix(value, plus)
      return "-" if value.__s__ < 0
      return " " if plus == 1
      return "+" if plus == 2
      ""
    end

    # VpToSpecialString, or nil for a finite nonzero value.
    def self.special_string(value, plus)
      case value.__s__
      when 0 then "NaN"
      when 3 then (plus == 1 ? " " : plus == 2 ? "+" : "") + "Infinity"
      when -3 then "-Infinity"
      when 1 then plus == 1 ? " 0.0" : plus == 2 ? "+0.0" : "0.0"
      when -1 then "-0.0"
      end
    end

    def self.to_e(value, group = 0, plus = 0)
      special = special_string(value, plus)
      return special if special
      digits = value.__m__.to_s
      if group > 0
        digits = digits.scan(/.{1,#{group}}/).join(" ")
      end
      "#{sign_prefix(value, plus)}0.#{digits}e#{value.__q__ + value.__n__}"
    end

    def self.to_f_string(value, group = 0, plus = 0)
      special = special_string(value, plus)
      return special if special
      digits = value.__m__.to_s
      point = value.__q__ + value.__n__
      if point <= 0
        whole = "0"
        fraction = "0" * -point + digits
      elsif value.__q__ >= 0
        whole = digits + "0" * value.__q__
        fraction = ""
      else
        whole = digits.byteslice(0, point)
        fraction = digits.byteslice(point, digits.bytesize - point)
      end
      if group > 0
        if point > 0
          head = whole.bytesize % group
          parts = []
          parts << whole.byteslice(0, head) if head > 0
          whole.byteslice(head, whole.bytesize - head).scan(/.{#{group}}/) { |part| parts << part }
          whole = parts.join(" ")
        end
        fraction = fraction.scan(/.{1,#{group}}/).join(" ")
      end
      fraction = "0" if fraction.empty?
      "#{sign_prefix(value, plus)}#{whole}.#{fraction}"
    end
  end
  private_constant :Vp__

  # --- BigDecimal's own methods ------------------------------------------------

  def self.interpret_loosely(string)
    string = String.__coerce__(string)
    Vp__.parse(string, self, false, true)
  end

  def self.mode(which, value = nil)
    flags = Vp__.int(which)
    if flags & EXCEPTION_ALL != 0
      current = Vp__.exception_mode
      return current if value.nil?
      unless true.equal?(value) || false.equal?(value)
        raise ArgumentError, "second argument must be true or false"
      end
      [EXCEPTION_INFINITY, EXCEPTION_NaN, EXCEPTION_UNDERFLOW, EXCEPTION_ZERODIVIDE].each do |flag|
        next if flags & flag == 0
        current = value ? current | flag : current & ~flag
      end
      Thread.current[Vp__::EXCEPTION_KEY] = current
      return current
    end
    if flags == ROUND_MODE
      return Vp__.rounding_mode if value.nil?
      mode = Vp__.rounding_mode_of(value)
      Thread.current[Vp__::ROUNDING_KEY] = mode
      return mode
    end
    raise TypeError, "first argument for BigDecimal.mode invalid"
  end

  def self.limit(digits = nil)
    current = Vp__.limit
    return current if digits.nil?
    digits = Vp__.int(digits)
    raise ArgumentError, "argument must be positive" if digits < 0
    Vp__.limit = digits
    current
  end

  def self.double_fig = Vp__::DOUBLE_FIG

  def self._load(string)
    string = String.__coerce__(string)
    raise ArgumentError, "string contains null byte" if string.include?("\0")
    bytes = string.b
    i = 0
    while i < bytes.bytesize
      byte = bytes.getbyte(i)
      i += 1
      break if byte == 58 # ':'
      unless byte >= 48 && byte <= 57
        raise TypeError, "load failed: invalid character in the marshaled string"
      end
    end
    Vp__.check(Vp__.parse(bytes.byteslice(i, bytes.bytesize - i), self, true, true))
  end

  def self.save_exception_mode
    mode = Vp__.exception_mode
    begin
      yield
    ensure
      Thread.current[Vp__::EXCEPTION_KEY] = mode
    end
  end

  def self.save_rounding_mode
    mode = Vp__.rounding_mode
    begin
      yield
    ensure
      Thread.current[Vp__::ROUNDING_KEY] = mode
    end
  end

  def self.save_limit
    limit = Vp__.limit
    begin
      yield
    ensure
      Vp__.limit = limit
    end
  end

  INFINITY = Vp__.infinity(1)
  NAN = Vp__.nan

  # VpCountPrecisionAndScale, in an ssize_t (a precision past 2**63
  # wraps, as the C's does).
  def precision
    return 0 unless @__s__.abs == 2
    point = @__q__ + @__n__
    return Vp__.ssize(-@__q__) if point <= 0
    Vp__.ssize(@__q__ >= 0 ? point : @__n__)
  end

  def scale
    return 0 unless @__s__.abs == 2
    @__q__ < 0 ? Vp__.ssize(-@__q__) : 0
  end

  def precision_scale = [precision, scale]

  def n_significant_digits
    @__s__.abs == 2 ? @__n__ : 0
  end

  def add(value, digits)
    __addsub__(value, Vp__.precision(digits), 1)
  end

  def sub(value, digits)
    __addsub__(value, Vp__.precision(digits), -1)
  end

  def mult(value, digits)
    __mult__(value, Vp__.precision(digits))
  end

  def div(value, digits = nil)
    __div2__(value, digits)
  end

  # BigDecimal_hash: the sign alone for a zero, NaN or infinity; equal
  # values have equal hashes, the digits being kept without trailing zeros.
  def hash
    return @__s__ unless @__s__.abs == 2
    @__m__.hash ^ (@__q__ * 4 + @__s__).hash
  end

  def to_s(format = nil)
    group = 0
    plus = 0
    fixed = false
    unless format.nil?
      if format.is_a?(String)
        bytes = format.b
        i = 0
        if bytes.getbyte(0) == 32
          plus = 1
          i = 1
        elsif bytes.getbyte(0) == 43
          plus = 2
          i = 1
        end
        while i < bytes.bytesize
          byte = bytes.getbyte(i)
          i += 1
          break if byte == 0
          next if Vp__.space?(byte)
          unless Vp__.digit?(byte)
            fixed = true if byte == 70 || byte == 102
            break
          end
          group = group * 10 + byte - 48
        end
      else
        group = Vp__.int(format)
        raise ArgumentError, "argument must be positive" if group <= 0
      end
    end
    string = fixed ? Vp__.to_f_string(self, group, plus) : Vp__.to_e(self, group, plus)
    String.new(string, encoding: Encoding::US_ASCII)
  end

  def to_i
    Vp__.check(self, true)
    # A zero with a stale exponent of 2 or more: the C multiplies "0" by
    # 10**-1.
    return Rational(0, 1) if @__s__.abs == 1 && @__e__
    return 0 unless @__s__.abs == 2
    return 0 if @__q__ + @__n__ <= 0
    value = @__q__ >= 0 ? @__m__ * 10**@__q__ : @__m__ / 10**-@__q__
    @__s__ < 0 ? -value : value
  end
  alias to_int to_i

  def to_r
    Vp__.check(self, true)
    return Rational(0, 1) unless @__s__.abs == 2
    numerator = @__s__ < 0 ? -@__m__ : @__m__
    if @__q__ < 0
      Rational(numerator, 10**-@__q__)
    else
      Rational(numerator * 10**@__q__, 1)
    end
  end

  def split
    sign = @__s__ == 0 ? 0 : (@__s__ < 0 ? -1 : 1)
    digits = case @__s__
             when 0 then "NaN"
             when 3, -3 then "Infinity"
             when 1, -1 then "0"
             else @__m__.to_s
             end
    [sign, digits.b, 10, exponent]
  end

  def +(value)
    return __coerced__(value, :+) unless __coercible__(value)
    __addsub__(value, 0, 1)
  end

  def -(value)
    return __coerced__(value, :-) unless __coercible__(value)
    __addsub__(value, 0, -1)
  end

  def +@ = self

  def -@
    Vp__.check(__with_sign__(-1))
  end

  def *(value)
    return __coerced__(value, :*) unless __coercible__(value)
    __mult__(value, 0)
  end

  def /(value)
    return __coerced__(value, :/) unless __coercible__(value)
    __div2__(value, 0)
  end

  def quo(value, digits = (omitted = true; nil))
    digits = Vp__.precision(digits) unless omitted
    return __div2__(value, digits) if digits && digits > 0
    self / value
  end

  def %(value)
    result = __divmod__(value, false)
    return __coerced__(value, :%) if result.nil?
    Vp__.check(result[1])
  end
  alias modulo %

  def remainder(value)
    result = __divmod__(value, true)
    return __coerced__(value, :remainder) if result.nil?
    Vp__.check(result[1])
  end

  def divmod(value)
    result = __divmod__(value, false)
    return __coerced__(value, :divmod) if result.nil?
    quotient = result[0]
    quotient = Vp__.check(quotient).to_i if quotient.is_a?(BigDecimal)
    [quotient, Vp__.check(result[1])]
  end

  # BigDecimal_clone, both names: a frozen value is its own copy.
  def dup = self
  alias clone dup

  def to_f
    case @__s__
    when 0 then return Float::NAN
    when 1 then return 0.0
    when -1 then return -0.0
    when 3 then return Float::INFINITY
    when -3 then return -Float::INFINITY
    end
    word = Vp__.word_exponent(@__q__ + @__n__) * 9
    if word > 308 + 9
      return __float_overflow__
    elsif word < -307 - 15
      return __float_underflow__
    end
    float = Float(Vp__.to_e(self))
    return __float_underflow__ if float == 0.0
    return __float_overflow__ if float.infinite?
    float
  end

  def abs
    Vp__.check(__with_sign__(@__s__ < 0 ? -1 : 1))
  end

  def fix
    return Vp__.check(__copy__) unless @__s__.abs == 2
    Vp__.check(Vp__.round_places(self, 0, ROUND_DOWN))
  end

  def round(*args)
    mode = nil
    places = 0
    to_integer = false
    case args.size
    when 0
      to_integer = true
    when 1
      if args[0].is_a?(Hash)
        mode = Vp__.rounding_option(args[0])
      else
        places = Vp__.int(args[0])
        to_integer = true if places < 1
      end
    when 2
      places = Vp__.int(args[0])
      mode = args[1].is_a?(Hash) ? Vp__.rounding_option(args[1]) : Vp__.rounding_mode_of(args[1])
    else
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
    end
    result = @__s__.abs == 2 ? Vp__.round_places(self, places, mode || Vp__.rounding_mode) : __copy__
    Vp__.check(result)
    to_integer ? result.to_i : result
  end

  def frac
    return Vp__.check(__copy__) unless @__s__.abs == 2
    return self if @__q__ + @__n__ <= 0
    return Vp__.zero(@__s__) if @__q__ >= 0
    divisor = 10**-@__q__
    Vp__.check(Vp__.finite(@__s__, @__m__ % divisor, @__q__))
  end

  def floor(digits = (omitted = true; nil))
    __truncate_floor_ceil__(omitted, digits, ROUND_FLOOR)
  end

  def ceil(digits = (omitted = true; nil))
    __truncate_floor_ceil__(omitted, digits, ROUND_CEILING)
  end

  def truncate(digits = (omitted = true; nil))
    __truncate_floor_ceil__(omitted, digits, ROUND_DOWN)
  end

  def <=>(value)
    other = Vp__.operand(value, self, 0, false)
    if other.nil?
      pair = __coerce_pair__(value, false)
      return nil if pair.nil?
      return pair[0] <=> pair[1]
    end
    Vp__.compare(self, other)
  end

  def ==(value)
    other = Vp__.operand(value, self, 0, false)
    if other.nil?
      pair = __coerce_pair__(value, false)
      return false if pair.nil?
      return pair[0] == pair[1] ? true : false
    end
    Vp__.compare(self, other) == 0
  end
  alias === ==
  alias eql? ==

  def <(value) = __relop__(value, :<) { |c| c < 0 }
  def <=(value) = __relop__(value, :<=) { |c| c <= 0 }
  def >(value) = __relop__(value, :>) { |c| c > 0 }
  def >=(value) = __relop__(value, :>=) { |c| c >= 0 }

  def zero? = @__s__.abs == 1

  def nonzero? = @__s__.abs == 1 ? nil : self

  def coerce(other)
    [Vp__.check(Vp__.operand(other, self, 0, true)), self]
  end

  def inspect = Vp__.to_e(self).b

  def exponent
    @__s__.abs == 2 ? @__q__ + @__n__ : 0
  end

  def sign = @__s__

  def nan? = @__s__ == 0

  def infinite?
    return 1 if @__s__ == 3
    return -1 if @__s__ == -3
    nil
  end

  def finite? = @__s__.abs != 3 && @__s__ != 0

  def _decimal_shift(shift)
    # NUM2SSIZET(rb_to_int(shift)).
    shift = Integer.__to_int__(shift)
    if shift > 9_223_372_036_854_775_807 || shift < -9_223_372_036_854_775_808
      raise RangeError, "bignum too big to convert into 'long long'"
    end
    return Vp__.check(self) unless @__s__.abs == 2 && shift != 0
    Vp__.check(Vp__.finite(@__s__, @__m__, @__q__ + shift))
  end

  def _dump(level = nil)
    "#{Vp__.words(self) * 9}:#{Vp__.to_e(self)}".b
  end

  private

  # is_coerceable_to_BigDecimal.
  def __coercible__(value)
    value.is_a?(BigDecimal) || value.is_a?(Integer) || value.is_a?(Float) || value.is_a?(Rational)
  end

  # rb_num_coerce_bin.
  def __coerced__(value, operator)
    pair = __coerce_pair__(value, true)
    pair[0].__send__(operator, pair[1])
  end

  # BigDecimalCmp for the relational operators.
  def __relop__(value, operator)
    other = Vp__.operand(value, self, 0, false)
    if other.nil?
      pair = __coerce_pair__(value, false)
      __cmp_failed__(value) if pair.nil?
      answer = pair[0].__send__(operator, pair[1])
      __cmp_failed__(value) if answer.nil?
      return answer
    end
    comparison = Vp__.compare(self, other)
    return false if comparison.nil?
    yield comparison
  end

  # BigDecimal_addsub_with_coerce.
  def __addsub__(value, precision, operation)
    other = Vp__.operand(value, self, precision, true)
    return Vp__.check(self) if @__s__ == 0
    return Vp__.check(other) if other.__s__ == 0
    limit = precision > 0 ? precision : Vp__.limit
    Vp__.check(Vp__.add(self, other, operation, limit))
  end

  # BigDecimal_mult_with_coerce.
  def __mult__(value, precision)
    other = Vp__.operand(value, self, precision, true)
    product = Vp__.multiply(self, other)
    product = if precision > 0
                Vp__.round_digits(product, precision, Vp__.rounding_mode)
              else
                Vp__.limit_round(product)
              end
    Vp__.check(product)
  end

  # BigDecimal_div2: `/` (digits 0), `div(value, digits)`, and
  # `div(value)` (digits nil), the floored quotient as an Integer.
  def __div2__(value, digits)
    if digits.nil?
      result = __divmod__(value, false)
      return __coerced__(value, :div) if result.nil?
      quotient = result[0]
      quotient = Vp__.check(quotient).to_i if quotient.is_a?(BigDecimal)
      return quotient
    end
    digits = Vp__.precision(digits)
    other = Vp__.operand(value, self, digits, true)
    if digits == 0
      limit = Vp__.limit
      digits = Vp__.ssize([precision, other.precision].max + Vp__::DOUBLE_FIG)
      digits = 2 * Vp__::DOUBLE_FIG if digits < 2 * Vp__::DOUBLE_FIG
      digits = limit if limit > 0 && limit < digits
    end
    Vp__.check(Vp__.divide(self, other, digits))
  end

  # BigDecimal_DoDivmod: [quotient, remainder], the quotient an Integer
  # or, for NaN and infinite quotients, a BigDecimal; nil when the operand
  # must be coerced.
  def __divmod__(value, truncate)
    other = Vp__.operand(value, self, 0, false)
    return nil if other.nil?
    s = @__s__
    t = other.__s__
    if s == 0 || t == 0 || (s.abs == 3 && t.abs == 3)
      return [Vp__.nan, Vp__.nan]
    end
    raise ZeroDivisionError, "divided by 0" if t.abs == 1
    if s.abs == 3
      return [Vp__.infinity((s < 0) == (t < 0) ? 1 : -1), Vp__.nan]
    end
    return [0, self] if s.abs == 1
    if t.abs == 3
      return [-1, other] if !truncate && (s < 0) != (t < 0)
      return [0, self]
    end
    Vp__.divmod(self, other, truncate)
  end

  def __truncate_floor_ceil__(omitted, digits, mode)
    places = omitted ? 0 : Vp__.int(digits)
    result = @__s__.abs == 2 ? Vp__.round_places(self, places, mode) : __copy__
    Vp__.check(result)
    omitted ? result.to_i : result
  end

  # VpAsgn(c, a, 10): a copy, a NaN or infinity without the words VpDivd
  # left in it.
  def __copy__
    case @__s__
    when 0 then Vp__.nan
    when 3, -3 then Vp__.infinity(@__s__)
    when 1, -1 then Vp__.zero(@__s__)
    else self
    end
  end

  # The value with its sign multiplied by `sign` (`-@`) or set to positive
  # (`abs`).
  def __with_sign__(sign)
    case @__s__
    when 0 then Vp__.nan
    when 3, -3 then Vp__.infinity(@__s__ * sign)
    when 1, -1 then Vp__.zero(@__s__ * sign)
    else Vp__.make(@__s__ * sign, @__m__, @__q__, @__n__)
    end
  end

  def __float_overflow__
    Vp__.exception(EXCEPTION_OVERFLOW, "BigDecimal to Float conversion")
    @__s__ < 0 ? -Float::INFINITY : Float::INFINITY
  end

  def __float_underflow__
    Vp__.exception(EXCEPTION_UNDERFLOW, "BigDecimal to Float conversion")
    @__s__ < 0 ? -0.0 : 0.0
  end
end

class BigDecimal
  # f_BigDecimal, a global function. Inside BigDecimal, so Vp__ is in scope.
  module ::Kernel
    def BigDecimal(value, digits = (omitted = true; nil), exception: true)
      unless true.equal?(exception) || false.equal?(exception)
        raise ArgumentError, "expected true or false as exception: #{exception.inspect}"
      end
      unless omitted
        digits = Integer.__to_int__(digits)
        if digits < 0
          return nil unless exception
          raise ArgumentError, "negative precision"
        end
      end
      Vp__.convert(value, digits, exception)
    end
    module_function :BigDecimal
  end
end
