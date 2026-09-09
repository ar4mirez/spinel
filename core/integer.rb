# Integer.
#
# The arithmetic operators, comparisons, `&`, `|`, `^`, `<<`, `>>`, `~` and
# `**` are primitives: they are fixnum bit patterns, and the JIT wants them as
# intrinsics. Everything below is Ruby.
class Integer
  def times
    return to_enum(:times) { self < 0 ? 0 : self } unless block_given?
    i = 0
    while i < self
      yield i
      i = i + 1
    end
    self
  end

  def upto(last)
    return to_enum(:upto, last) { __span__(last, 1) } unless block_given?
    i = self
    while i <= last
      yield i
      i = i + 1
    end
    self
  end

  def downto(first)
    return to_enum(:downto, first) { __span__(first, -1) } unless block_given?
    i = self
    while i >= first
      yield i
      i = i - 1
    end
    self
  end

  def zero?
    self == 0
  end

  def positive?
    self > 0
  end

  def negative?
    self < 0
  end

  def even?
    (self % 2) == 0
  end

  def odd?
    (self % 2) != 0
  end

  def succ
    self + 1
  end

  def next
    self + 1
  end

  def pred
    self - 1
  end

  def abs
    self < 0 ? -self : self
  end

  def integer?
    true
  end

  def to_i
    self
  end

  # `1.eql?(1.0)` is false: `eql?` does not convert, and `hash` must agree.
  def eql?(other)
    other.is_a?(Integer) && self == other
  end

  def to_int
    self
  end

  # Multiplying by a Float is what promotes, and the VM's fast path already
  # does that without dispatching — so this is Ruby expressing a conversion
  # rather than a primitive standing in for one.
  def to_f
    self * 1.0
  end

  # Bit `n`, counting from the least significant. Negative integers read as
  # two's complement extended forever, so `-1[99]` is 1; a negative index is 0
  # rather than an error.
  #
  # Three shapes: `n[i]` is one bit, `n[i, len]` and `n[range]` are a field of
  # them, shifted down to the bottom. A negative length means "no limit", and a
  # beginless range is the one shape Ruby refuses — the field would be infinite.
  # All measured on ruby 4.0.6.
  def [](index, *length)
    if length.size > 1
      raise ArgumentError, "wrong number of arguments (given #{length.size + 1}, expected 1..2)"
    end
    return __bit_field__(index, length[0]) unless length.empty?
    if index.is_a?(Range)
      if index.begin.nil?
        raise ArgumentError, "The beginless range for Integer#[] results in infinity"
      end
      last = index.end
      if last.nil?
        width = -1
      else
        if last < index.begin
          # An upper bound below the lower one is ignored rather than empty:
          # `0b101001101[4..1]` is every bit from 4 up, and `[-4..-5]` is the
          # whole number shifted up by 4. Measured — the arithmetic width would
          # be 0 for `-4..-5`, which is a different answer.
          width = -1
        else
          width = last - index.begin + (index.exclude_end? ? 0 : 1)
        end
      end
      return __bit_field__(index.begin, width)
    end
    index = __to_index__(index)
    return 0 if index < 0
    (self >> index) & 1
  end

  # `len` bits starting at `from`, as a number. A negative `len` takes every
  # remaining bit, which is what an endless range asks for.
  #
  # A negative `from` moves the field *up* into the more significant bits rather
  # than being clamped to zero — `0b000001[-3, 4]` is `0b1000`. That is the one
  # place the field form and the single-bit form disagree: `5[-1]` is still 0.
  def __bit_field__(from, len)
    from = __to_index__(from)
    len = __to_index__(len)
    shifted = from < 0 ? (self << -from) : (self >> from)
    return shifted if len < 0
    return 0 if len == 0
    shifted & ((1 << len) - 1)
  end

  def __to_index__(value)
    return value if value.is_a?(Integer)
    unless value.respond_to?(:to_int)
      raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into Integer"
    end
    value.to_int
  end

  # Float division. `to_f` first so an Integer operand does not floor.
  #
  # ponytail: `1.fdiv(0)` is Infinity in Ruby and reports a missing `Float#/`
  # here, because this VM has only flonums and no boxed infinity — #18. Every
  # finite case is right.
  def fdiv(other)
    to_f / other
  end

  # Euclid. `gcd` is always positive, and `gcd(0)` is the other operand's
  # magnitude — `0.gcd(0)` is 0.
  def gcd(other)
    unless other.is_a?(Integer)
      raise TypeError, "not an integer"
    end
    a = abs
    b = other.abs
    while b != 0
      a, b = b, a % b
    end
    a
  end

  def lcm(other)
    unless other.is_a?(Integer)
      raise TypeError, "not an integer"
    end
    return 0 if self == 0 || other == 0
    (self * other).abs / gcd(other)
  end

  def gcdlcm(other)
    [gcd(other), lcm(other)]
  end

  # Least significant digit first. `0.digits` is `[0]`, not `[]`.
  def digits(base = 10)
    unless base.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{base.class} into Integer"
    end
    raise ArgumentError, "negative radix" if base < 0
    raise ArgumentError, "invalid radix #{base}" if base < 2
    if self < 0
      raise ArgumentError, "digits of a negative number are out of domain"
    end
    return [0] if self == 0
    out = []
    n = self
    while n > 0
      out.push(n % base)
      n = n / base
    end
    out
  end

  # The position of the highest bit that differs from the sign bit, so `-1` and
  # `0` are both 0 and `-256` is 8. `~self` folds the negative case onto the
  # positive one exactly.
  def bit_length
    n = self < 0 ? ~self : self
    count = 0
    while n > 0
      n = n >> 1
      count = count + 1
    end
    count
  end

  # Floored division, which is what `/` already does for two Integers — but
  # `div` floors for a Float operand too and answers an Integer.
  #
  # ponytail: the Float operand reports a missing `Float#floor` rather than
  # answering, because this VM has no float-to-integer primitive — #18. The
  # Integer case, which is every case ruby/spec reaches without a Float, is
  # right.
  def div(other)
    if other == 0
      raise ZeroDivisionError, "divided by 0"
    end
    return self / other if other.is_a?(Integer)
    (self / other).floor
  end

  def divmod(other)
    [div(other), self % other]
  end

  def modulo(other)
    self % other
  end

  # Sign of the receiver, unlike `%`, which takes the sign of the divisor.
  def remainder(other)
    if other == 0
      raise ZeroDivisionError, "divided by 0"
    end
    r = abs % other.abs
    self < 0 ? -r : r
  end

  # Bytes needed to hold the magnitude, with the machine word as the floor: `1`
  # and `2**63` are both 8, `2**64` is 9. Measured — the spec's own comment
  # describes it as the n with `256 ** n <= abs < 256 ** (n + 1)`.
  def size
    bytes = (bit_length + 7) / 8
    bytes < 8 ? 8 : bytes
  end

  # How many steps an `upto`/`downto` enumerator will take, or 0 when the
  # endpoint is already passed. Raises rather than answering when the two are
  # not comparable at all, which is what asking a bad enumerator for its size
  # does.
  def __span__(other, direction)
    cmp = (self <=> other)
    if cmp.nil?
      raise ArgumentError, "comparison of Integer with #{other.inspect} failed"
    end
    span = direction > 0 ? other - self + 1 : self - other + 1
    span < 0 ? 0 : span
  end

  # `pow(e)` is `**`; `pow(e, m)` is modular exponentiation, done by squaring so
  # the intermediate never grows past `m * m`.
  def pow(exponent, *rest)
    if rest.size > 1
      raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 1..2)"
    end
    return self ** exponent if rest.empty?
    modulus = rest[0]
    unless modulus.is_a?(Integer)
      raise TypeError,
            "Integer#pow() 2nd argument not allowed unless all arguments are integers"
    end
    raise ZeroDivisionError, "divided by 0" if modulus == 0
    if exponent < 0
      raise RangeError, "Integer#pow() 1st argument cannot be negative when 2nd argument specified"
    end
    result = 1
    base = self % modulus
    e = exponent
    while e > 0
      result = (result * base) % modulus if (e & 1) == 1
      base = (base * base) % modulus
      e = e >> 1
    end
    result
  end

  # An Integer is already whole, so these only do anything for a negative digit
  # count, which rounds to a multiple of a power of ten.
  #
  # `round` goes half away from zero — `25.round(-1)` is 30, not the 20 a
  # banker's rule would give. Measured.
  def round(digits = 0)
    digits = __round_digits__(digits)
    return self if digits >= 0
    step = 10 ** (-digits)
    # On the magnitude, then signed back. Shifting the signed value by half and
    # floor-dividing looks equivalent and is not: it rounds -42 to -10**100
    # rather than 0, because floor division of a negative already rounds away.
    quotient = (abs + step / 2) / step
    rounded = quotient * step
    self < 0 ? -rounded : rounded
  end

  def ceil(digits = 0)
    digits = __round_digits__(digits)
    return self if digits >= 0
    step = 10 ** (-digits)
    ((self + step - 1) / step) * step
  end

  def floor(digits = 0)
    digits = __round_digits__(digits)
    return self if digits >= 0
    step = 10 ** (-digits)
    (self / step) * step
  end

  # Toward zero, which differs from `floor` for a negative receiver.
  def truncate(digits = 0)
    digits = __round_digits__(digits)
    return self if digits >= 0
    step = 10 ** (-digits)
    (abs / step) * step * (self < 0 ? -1 : 1)
  end

  # The digit count `round` and friends take: an Integer, and one that would fit
  # in the C int CRuby converts it to.
  def __round_digits__(digits)
    unless digits.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{digits.class} into Integer"
    end
    if digits >= 2147483648 || digits < -2147483648
      raise RangeError, "integer #{digits} too big to convert to 'int'"
    end
    digits
  end

  # Integer square root by Newton's method: the largest n with n*n <= self.
  def self.sqrt(value)
    unless value.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{value.class} into Integer"
    end
    if value < 0
      raise ArgumentError, "Numerical argument is out of domain - \"isqrt\""
    end
    return value if value < 2
    guess = value
    better = (guess + 1) / 2
    while better < guess
      guess = better
      better = (guess + value / guess) / 2
    end
    guess
  end

  DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz"

  def to_s(base = 10)
    if base < 2 || base > 36
      raise ArgumentError, "invalid radix " + base.to_s
    end
    return "0" if self == 0
    n = abs
    out = ""
    while n > 0
      out = DIGITS[n % base] + out
      n = n / base
    end
    self < 0 ? "-" + out : out
  end

  def inspect
    to_s
  end
end
