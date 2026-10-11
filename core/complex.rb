# Complex — a real part and an imaginary one (#34).
#
# Two real numbers, kept exactly as they were given: `Complex(1, 0.0)` has an
# Integer and a Float in it, and prints as `(1+0.0i)`. The arithmetic is the
# schoolbook kind over whatever the parts are, so two Complexes of Integers
# multiply to a Complex of Integers and divide to one of Rationals. It is all
# Ruby.
#
# `Complex.new` does not exist, in Ruby or here: `Kernel#Complex`,
# `Complex.rect` and `Complex.polar` are the ways in.
class Complex < Numeric
  class << self
    undef_method :new

    def __make__(real, imaginary)
      allocate.__set__(real, imaginary)
    end

    # A compiled literal: `3i` is the number with no real part.
    def __literal__(imaginary)
      __make__(0, imaginary)
    end

    def rect(real, imaginary = 0)
      __make__(__check_real__(real), __check_real__(imaginary))
    end
    alias rectangular rect

    # From a length and an angle. The angles that land on an axis are
    # answered exactly, as CRuby answers them, rather than through a cosine
    # that is not quite zero.
    def polar(length, angle = 0)
      length = __check_real__(length)
      angle = __check_real__(angle)
      if angle == 0
        return __make__(length, angle.is_a?(Float) ? angle * length : 0.0) unless length.is_a?(Float) && angle.is_a?(Float)
      end
      if angle.is_a?(Float)
        return __make__(-length, 0.0) if angle == Math::PI
        return __make__(0.0, length) if angle == Math::PI / 2
      end
      __make__(length * Math.cos(angle), length * Math.sin(angle))
    end

    # A real number, or a Complex that is one: its imaginary part is zero.
    def __check_real__(value)
      return value.real if value.is_a?(Complex) && value.imaginary == 0
      unless value.is_a?(Numeric) && value.real?
        raise TypeError, "not a real"
      end
      value
    end

    # One argument of `Kernel#Complex`, as a number.
    def __convert__(value)
      return value if Numeric === value
      if NilClass === value
        raise TypeError, "can't convert nil into Complex"
      end
      if String === value
        parsed = __parse__(value, true)
        return parsed unless parsed.nil?
        raise ArgumentError, "invalid value for convert(): " + value.inspect
      end
      if value.respond_to?(:to_c)
        converted = value.to_c
        return converted if Numeric === converted
      end
      raise TypeError, "can't convert " + value.class.to_s + " into Complex"
    end

    NUMBER = '(?:\d+(?:_\d+)*(?:\.\d+(?:_\d+)*)?(?:[eE][+-]?\d+(?:_\d+)*)?(?:\/\d+(?:_\d+)*)?)'
    private_constant :NUMBER

    # One real number out of a string: an Integer, a Rational when it has a
    # `/`, a Float when it has a point or an exponent.
    def __number__(text)
      text = text.delete("_")
      negative = text.start_with?("-")
      text = text[1, text.size - 1] if text.start_with?("-") || text.start_with?("+")
      value = if text.include?("/")
                top, bottom = text.split("/")
                top = top.include?(".") || top.include?("e") || top.include?("E") ? top.to_f : top.to_i
                top.is_a?(Float) ? top / bottom.to_i : Rational(top, bottom.to_i)
              elsif text.include?(".") || text.include?("e") || text.include?("E")
                text.to_f
              else
                text.to_i
              end
      negative ? -value : value
    end

    # `real`, `imag i`, `real±imag i` or `length@angle`. Strict wants the
    # whole string; lenient reads what leads it and answers nil only when
    # nothing does.
    def __parse__(text, strict)
      raise ArgumentError, "string contains null byte" if strict && text.include?("\0")
      tail = strict ? '\s*\z' : ""
      head = '\A\s*'
      if (match = Regexp.new(head + '([+-]?' + NUMBER + ')@([+-]?' + NUMBER + ')' + tail).match(text))
        return polar(__number__(match[1]), __number__(match[2]))
      end
      if (match = Regexp.new(head + '([+-]?' + NUMBER + ')([+-])(' + NUMBER + ')?[ijIJ]' + tail).match(text))
        imaginary = match[3].nil? ? 1 : __number__(match[3])
        return __make__(__number__(match[1]), match[2] == "-" ? -imaginary : imaginary)
      end
      if (match = Regexp.new(head + '([+-]?)(' + NUMBER + ')?[ijIJ]' + tail).match(text))
        imaginary = match[2].nil? ? 1 : __number__(match[2])
        return __make__(0, match[1] == "-" ? -imaginary : imaginary)
      end
      if (match = Regexp.new(head + '([+-]?' + NUMBER + ')' + tail).match(text))
        return __make__(__number__(match[1]), 0)
      end
      nil
    end

    # A Rational that is a whole number, as the Integer it is: what makes
    # `Complex(3, 4) / Complex(3, 4)` print as `(1+0i)`.
    def __whole__(value)
      value.is_a?(Rational) && value.denominator == 1 ? value.numerator : value
    end
  end

  def __set__(real, imaginary)
    @__real__ = real
    @__imaginary__ = imaginary
    freeze
  end

  def real = @__real__
  def imaginary = @__imaginary__
  alias imag imaginary

  def real? = false

  def rect
    [@__real__, @__imaginary__]
  end
  alias rectangular rect

  def abs
    Math.hypot(@__real__, @__imaginary__)
  end
  alias magnitude abs

  def abs2
    @__real__ * @__real__ + @__imaginary__ * @__imaginary__
  end

  def arg
    Math.atan2(@__imaginary__, @__real__)
  end
  alias angle arg
  alias phase arg

  def polar
    [abs, arg]
  end

  def conjugate
    Complex.__make__(@__real__, -@__imaginary__)
  end
  alias conj conjugate

  def -@
    Complex.__make__(-@__real__, -@__imaginary__)
  end

  # -- arithmetic -----------------------------------------------------------

  def +(other)
    if other.is_a?(Complex)
      Complex.__make__(@__real__ + other.real, @__imaginary__ + other.imaginary)
    elsif other.is_a?(Numeric) && other.real?
      Complex.__make__(@__real__ + other, @__imaginary__)
    else
      pair = __coerce_pair__(other, true)
      pair[0] + pair[1]
    end
  end

  def -(other)
    if other.is_a?(Complex)
      Complex.__make__(@__real__ - other.real, @__imaginary__ - other.imaginary)
    elsif other.is_a?(Numeric) && other.real?
      Complex.__make__(@__real__ - other, @__imaginary__)
    else
      pair = __coerce_pair__(other, true)
      pair[0] - pair[1]
    end
  end

  def *(other)
    if other.is_a?(Complex)
      Complex.__make__(@__real__ * other.real - @__imaginary__ * other.imaginary,
                       @__real__ * other.imaginary + @__imaginary__ * other.real)
    elsif other.is_a?(Numeric) && other.real?
      Complex.__make__(@__real__ * other, @__imaginary__ * other)
    else
      pair = __coerce_pair__(other, true)
      pair[0] * pair[1]
    end
  end

  # Exact parts divide exactly, to Rationals; with a Float anywhere the
  # division is the scaled one that does not overflow on the way.
  def /(other)
    if other.is_a?(Complex)
      floats = @__real__.is_a?(Float) || @__imaginary__.is_a?(Float) ||
               other.real.is_a?(Float) || other.imaginary.is_a?(Float)
      unless floats
        scale = other.abs2
        raise ZeroDivisionError, "divided by 0" if scale == 0
        top = self * other.conjugate
        return Complex.__make__(Complex.__whole__(top.real.quo(scale)), Complex.__whole__(top.imaginary.quo(scale)))
      end
      r = other.real
      i = other.imaginary
      if r.abs >= i.abs
        ratio = i.to_f / r
        bottom = r * (1 + ratio * ratio)
        Complex.__make__((@__real__ + @__imaginary__ * ratio) / bottom, (@__imaginary__ - @__real__ * ratio) / bottom)
      else
        ratio = r.to_f / i
        bottom = i * (1 + ratio * ratio)
        Complex.__make__((@__real__ * ratio + @__imaginary__) / bottom, (@__imaginary__ * ratio - @__real__) / bottom)
      end
    elsif other.is_a?(Numeric) && other.real?
      if other.is_a?(Float) || @__real__.is_a?(Float) || @__imaginary__.is_a?(Float)
        return Complex.__make__(@__real__ / other.to_f, @__imaginary__ / other.to_f) if other.is_a?(Float)
        raise ZeroDivisionError, "divided by 0" if other == 0 && !(@__real__.is_a?(Float) && @__imaginary__.is_a?(Float))
      end
      raise ZeroDivisionError, "divided by 0" if other == 0 && !@__real__.is_a?(Float) && !@__imaginary__.is_a?(Float)
      Complex.__make__(Complex.__whole__(@__real__.quo(other)), Complex.__whole__(@__imaginary__.quo(other)))
    else
      pair = __coerce_pair__(other, true)
      pair[0].quo(pair[1])
    end
  end
  alias quo /

  def fdiv(other)
    Complex.__make__(@__real__.fdiv(other), @__imaginary__.fdiv(other))
  end

  # A whole power is repeated multiplication and stays exact. Any other goes
  # round through the length and the angle.
  def **(other)
    if other.is_a?(Rational) && other.denominator == 1
      other = other.numerator
    end
    if other.is_a?(Complex) && other.imaginary == 0 && !other.imaginary.is_a?(Float)
      other = other.real
    end
    return Complex.__make__(1, 0) if other == 0 && !other.is_a?(Float)
    if other.is_a?(Integer)
      if other < 0
        return Complex.__make__(1, 0) / (self**-other)
      end
      answer = Complex.__make__(1, 0)
      base = self
      while other > 0
        answer = answer * base if other.odd?
        other = other / 2
        base = base * base if other > 0
      end
      return answer
    end
    if other.is_a?(Complex)
      length, angle = polar
      log = Math.log(length)
      return Complex.polar(Math.exp(other.real * log - other.imaginary * angle),
                           angle * other.real + other.imaginary * log)
    end
    if other.is_a?(Numeric) && other.real?
      length, angle = polar
      return Complex.polar(length**other, angle * other)
    end
    pair = __coerce_pair__(other, true)
    pair[0]**pair[1]
  end

  # -- comparison -----------------------------------------------------------

  def ==(other)
    if other.is_a?(Complex)
      @__real__ == other.real && @__imaginary__ == other.imaginary
    elsif other.is_a?(Numeric) && other.real?
      @__imaginary__ == 0 && @__real__ == other
    else
      other == self ? true : false
    end
  end

  # There is an order only along the real line.
  def <=>(other)
    if other.is_a?(Complex)
      return nil unless @__imaginary__ == 0 && other.imaginary == 0
      @__real__ <=> other.real
    elsif other.is_a?(Numeric) && other.real?
      @__imaginary__ == 0 ? @__real__ <=> other : nil
    end
  end

  def eql?(other)
    other.is_a?(Complex) && @__real__.class.equal?(other.real.class) &&
      @__imaginary__.class.equal?(other.imaginary.class) && self == other
  end

  def hash
    [Complex, @__real__, @__imaginary__].hash
  end

  def coerce(other)
    if other.is_a?(Complex)
      [other, self]
    elsif other.is_a?(Numeric) && other.real?
      [Complex.__make__(other, 0), self]
    else
      raise TypeError, other.class.to_s + " can't be coerced into Complex"
    end
  end

  def finite?
    @__real__.finite? && @__imaginary__.finite?
  end

  def infinite?
    @__real__.infinite? || @__imaginary__.infinite? ? 1 : nil
  end

  def zero?
    @__real__ == 0 && @__imaginary__ == 0
  end

  # -- conversion -----------------------------------------------------------

  # Only a Complex that is exactly on the real line is a real number.
  def __real_only__(what)
    unless !@__imaginary__.is_a?(Float) && @__imaginary__ == 0
      raise RangeError, "can't convert " + to_s + " into " + what
    end
    @__real__
  end

  def to_i = __real_only__("Integer").to_i
  def to_f = __real_only__("Float").to_f
  # `to_r` alone lets a Float zero pass. Measured.
  def to_r
    return @__real__.to_r if @__imaginary__.is_a?(Float) && @__imaginary__ == 0
    __real_only__("Rational").to_r
  end
  def to_c = self

  def rationalize(*tolerance)
    __real_only__("Rational").rationalize(*tolerance)
  end

  def denominator
    @__real__.denominator.lcm(@__imaginary__.denominator)
  end

  def numerator
    below = denominator
    Complex.__make__(@__real__.numerator * (below / @__real__.denominator),
                     @__imaginary__.numerator * (below / @__imaginary__.denominator))
  end

  # `real±imag i`, with a `*` before the `i` when what precedes it does not
  # end in a digit: `1+Infinity*i`, `((1/2)+(3/4)*i)`.
  def __format__(real, imaginary)
    negative = if @__imaginary__.is_a?(Float)
                 @__imaginary__ < 0 || (@__imaginary__ == 0 && 1.0 / @__imaginary__ < 0)
               else
                 @__imaginary__ < 0
               end
    text = real + (negative ? "-" : "+") + imaginary
    last = text.getbyte(text.bytesize - 1)
    text = text + "*" unless last >= 48 && last <= 57
    text + "i"
  end

  def to_s
    __format__(@__real__.to_s, (@__imaginary__.is_a?(Float) && @__imaginary__.nan? ? @__imaginary__ : @__imaginary__.abs).to_s)
  end

  def inspect
    "(" + __format__(@__real__.inspect, (@__imaginary__.is_a?(Float) && @__imaginary__.nan? ? @__imaginary__ : @__imaginary__.abs).inspect) + ")"
  end

  def marshal_dump
    [@__real__, @__imaginary__]
  end
  private :marshal_dump

  # What a number on a plane has no meaning for.
  undef_method :<, :<=, :>, :>=, :between?, :clamp, :%, :div, :divmod, :floor, :ceil,
               :modulo, :remainder, :round, :truncate, :positive?, :negative?

  I = __make__(0, 1)
end

module Kernel
  # `Complex(real)`, `Complex(real, imaginary)`: the number, from numbers or
  # from a String. `exception: false` answers nil where it would raise.
  def Complex(first, second = :__none__, exception: true)
    begin
      first = Complex.__convert__(first)
      # What is not real is taken as it is: a Complex, or a program's own.
      return first.real? ? Complex.__make__(first, 0) : first if :__none__.equal?(second)
      second = Complex.__convert__(second)
      if first.real? && second.real?
        Complex.__make__(first, second)
      else
        # `first + second * i`, which is what two Complexes mean here.
        first + second * Complex::I
      end
    rescue TypeError, ArgumentError
      raise if exception
      nil
    end
  end
end

class Numeric
  def i = Complex.__make__(0, self)
  def to_c = Complex.__make__(self, 0)
  def imaginary = 0
  alias imag imaginary

  def rect
    [self, 0]
  end
  alias rectangular rect

  # A real number points along the axis one way or the other.
  def arg
    self < 0 ? Math::PI : 0
  end
  alias angle arg
  alias phase arg

  def polar
    [abs, arg]
  end

  def conjugate = self
  alias conj conjugate
end

class Float
  # NaN has no direction, and negative zero points backward.
  def arg
    return self if nan?
    self < 0 || (self == 0 && 1.0 / self < 0) ? Math::PI : 0
  end
  alias angle arg
  alias phase arg
end

class String
  # What leads the string, read as a Complex; zero when nothing does.
  def to_c
    Complex.__parse__(self, false) || Complex.__make__(0, 0)
  end
end

class NilClass
  def to_c = Complex.__make__(0, 0)
end
