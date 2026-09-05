# Integer.
#
# The arithmetic operators, comparisons, `&`, `|`, `^`, `<<`, `>>`, `~` and
# `**` are primitives: they are fixnum bit patterns, and the JIT wants them as
# intrinsics. Everything below is Ruby.
class Integer
  def times
    i = 0
    while i < self
      yield i
      i = i + 1
    end
    self
  end

  def upto(last)
    i = self
    while i <= last
      yield i
      i = i + 1
    end
    self
  end

  def downto(first)
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

  def <=>(other)
    return nil unless other.is_a?(Numeric)
    return -1 if self < other
    return 1 if self > other
    0
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
