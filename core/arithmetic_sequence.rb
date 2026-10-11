# Enumerator::ArithmeticSequence — `1.step(10, 3)` and `(1..10).step(3)` as
# values: a first term, a last, and the distance between terms.
#
# It is what `Numeric#step`, `Range#step` and `Range#%` answer without a
# block, and the same walk is what they run with one. The walk has two forms.
# Exact numbers are stepped by adding. Floats are not: adding 0.1 ten times
# does not arrive at 1.0, so the number of terms is worked out first and each
# term is `begin + i * step`, which is CRuby's `ruby_float_step`.
class Enumerator
  class ArithmeticSequence < Enumerator
    class << self
      undef_method :new

      # `how` is what `inspect` needs to print the call that made it.
      def __make__(first, last, step, exclude_end, how)
        allocate.__init_sequence__(first, last, step, exclude_end, how)
      end

      # How many Float terms there are, allowing for the error the division
      # carries.
      def __float_count__(first, last, step, exclude_end)
        span = (last - first) / step
        error = (first.abs + last.abs + (last - first).abs) / step.abs * Float::EPSILON
        if step.infinite?
          return (step > 0 ? first <= last : first >= last) ? 1 : 0
        end
        error = 0.5 if error > 0.5
        if exclude_end
          return 0 if span <= 0
          count = span < 1 ? 0 : (span - error).floor
          edge = (count + 1) * step + first
          count = count + 1 if (first < last && edge < last) || (first > last && edge > last)
          return count + 1
        end
        return 0 if span < 0
        (span + error).floor + 1
      end

      # Every term, in order. `last` nil is no end at all.
      def __walk__(first, last, step, exclude_end)
        raise ArgumentError, "step can't be 0" if step == 0
        if first.is_a?(Float) || last.is_a?(Float) || step.is_a?(Float)
          first = first.to_f
          step = step.to_f
          if last.nil?
            index = 0
            while true
              yield first + index * step
              index = index + 1
            end
          end
          last = last.to_f
          # An infinite distance in the direction of travel never ends, and
          # one that is not a number — infinity to itself — never starts.
          if step.infinite?.nil?
            span = (last - first) / step
            return if span.nan?
            unless span.infinite?.nil?
              return if span < 0
              index = 0
              while true
                yield first + index * step
                index = index + 1
              end
            end
          end
          count = __float_count__(first, last, step, exclude_end)
          # One term at most under an infinite step, and it is the first:
          # zero times infinity is not a number.
          unless step.infinite?.nil?
            yield first if count > 0
            return
          end
          index = 0
          while index < count
            term = index * step + first
            term = last if !exclude_end && (step >= 0 ? last < term : last > term)
            yield term
            index = index + 1
          end
          return
        end
        term = first
        if last.nil?
          while true
            yield term
            term = term + step
          end
        end
        if step > 0
          while exclude_end ? term < last : term <= last
            yield term
            term = term + step
          end
        else
          while exclude_end ? term > last : term >= last
            yield term
            term = term + step
          end
        end
      end

      def __count__(first, last, step, exclude_end)
        raise ArgumentError, "step can't be 0" if step == 0
        return Float::INFINITY if last.nil?
        if first.is_a?(Float) || last.is_a?(Float) || step.is_a?(Float)
          if step.to_f.infinite?.nil? && !last.to_f.infinite?.nil? && (last > 0) == (step > 0)
            return Float::INFINITY
          end
          return __float_count__(first.to_f, last.to_f, step.to_f, exclude_end)
        end
        return 0 if step > 0 ? first > last : first < last
        count = ((last - first) / step).floor
        count = count - 1 if exclude_end && first + count * step == last
        count + 1
      end
    end

    def __init_sequence__(first, last, step, exclude_end, how)
      __init_for__(self, :each, [], nil)
      @begin = first
      @end = last
      @step = step
      @exclude_end = exclude_end
      @how = how
      self
    end

    def begin = @begin
    def end = @end
    def step = @step
    def exclude_end? = @exclude_end

    def each(&block)
      return self if block.nil?
      ArithmeticSequence.__walk__(@begin, @end, @step, @exclude_end, &block)
      self
    end

    def size
      ArithmeticSequence.__count__(@begin, @end, @step, @exclude_end)
    end

    def first(count = nil)
      if count.nil?
        each { |term| return term }
        return nil
      end
      count = Integer.__index__(count)
      raise ArgumentError, "attempt to take negative size" if count < 0
      out = []
      return out if count == 0
      each do |term|
        out.push(term)
        break if out.size == count
      end
      out
    end

    def last(count = nil)
      if @end.nil?
        raise RangeError, "cannot get the last element of endless arithmetic sequence"
      end
      all = to_a
      return all.last if count.nil?
      count = Integer.__index__(count)
      raise ArgumentError, "negative array size" if count < 0
      all.last(count)
    end

    def ==(other)
      other.is_a?(ArithmeticSequence) && @begin == other.begin && @end == other.end &&
        @step == other.step && @exclude_end == other.exclude_end?
    end
    alias === ==
    alias eql? ==

    def hash
      [ArithmeticSequence, @begin, @end, @step, @exclude_end].hash
    end

    # The call that would make it again.
    def inspect
      if @how.is_a?(Array)
        receiver, positional, keywords = @how
        parts = positional.map { |value| value.inspect }
        keywords.each { |key, value| parts.push(key.to_s + ": " + value.inspect) }
        return "(" + receiver.inspect + ".step" + (parts.empty? ? "" : "(" + parts.join(", ") + ")") + ")"
      end
      range = "(" + @begin.inspect + (@exclude_end ? "..." : "..") + (@end.nil? ? "" : @end.inspect) + ")"
      return "(" + range + ".step)" if @how == :step_default
      "(" + range + "." + @how.to_s + "(" + @step.inspect + "))"
    end
    alias to_s inspect
  end
end

class Numeric
  # From this number toward `limit`, `step` at a time: `1.step(10, 3)`, or
  # by name, `1.step(by: 3, to: 10)`. With a block each term is yielded and
  # the number is answered; without one, the sequence itself.
  def step(*positional, **keywords, &block)
    if positional.size > 2
      raise ArgumentError, "wrong number of arguments (given " + positional.size.to_s + ", expected 0..2)"
    end
    unknown = keywords.keys.reject { |key| key == :by || key == :to }
    unless unknown.empty?
      raise ArgumentError, "unknown keyword" + (unknown.size > 1 ? "s" : "") + ": " +
                           unknown.map { |key| key.inspect }.join(", ")
    end
    raise ArgumentError, "to is given twice" if positional.size >= 1 && keywords.key?(:to)
    raise ArgumentError, "step is given twice" if positional.size >= 2 && keywords.key?(:by)
    limit = positional.size >= 1 ? positional[0] : keywords[:to]
    stride = if positional.size >= 2
               positional[1]
             elsif keywords.key?(:by)
               keywords[:by]
             else
               1
             end
    if block.nil?
      stride = 1 if stride.nil?
      # A step that is not a number makes no sequence. It is an ordinary
      # Enumerator, which says so when it is asked for its size or run.
      unless stride.is_a?(Numeric)
        arguments = keywords.empty? ? positional : positional + [keywords]
        return Enumerator.__for__(self, :step, arguments, -> { stride < 0 })
      end
      return Enumerator::ArithmeticSequence.__make__(self, limit, stride, false, [self, positional, keywords])
    end
    raise TypeError, "step must be numeric" if stride.nil?
    raise ArgumentError, "step can't be 0" if stride == 0
    # Asked for its sign first, which is where what is not a number says so.
    descending = stride < 0
    if limit.nil?
      Enumerator::ArithmeticSequence.__walk__(self, nil, stride, false, &block)
    else
      # And the limit for its order, for the same reason.
      descending ? self >= limit : self <= limit
      Enumerator::ArithmeticSequence.__walk__(self, limit, stride, false, &block)
    end
    self
  end
end

class Range
  # Every `stride`-th element. A numeric range is an arithmetic sequence; a
  # String range counts along `each`; anything else is stepped by `+`.
  def step(stride = nil, &block)
    first = self.begin
    last = self.end
    numeric = first.is_a?(Numeric) || (first.nil? && last.is_a?(Numeric))
    if numeric
      how = stride.nil? ? :step_default : :step
      stride = 1 if stride.nil?
      # What is not a number is asked to become one beside one.
      unless stride.is_a?(Numeric)
        if first.nil?
          raise ArgumentError, "#step for non-numeric beginless ranges is meaningless"
        end
        unless stride.respond_to?(:coerce)
          raise TypeError, "no implicit conversion of " + stride.class.to_s + " into Integer"
        end
        stride = stride.coerce(0)[1]
      end
      raise ArgumentError, "step can't be 0" if stride == 0
      if block.nil?
        return Enumerator::ArithmeticSequence.__make__(first, last, stride, exclude_end?, how)
      end
      raise ArgumentError, "#step iteration for beginless ranges is meaningless" if first.nil?
      Enumerator::ArithmeticSequence.__walk__(first, last, stride, exclude_end?, &block)
      return self
    end
    if first.is_a?(String) && stride.is_a?(Float)
      raise TypeError, "no implicit conversion to float from string"
    end
    if first.is_a?(String) && (stride.nil? || stride.is_a?(Integer) || stride.respond_to?(:to_int))
      stride = stride.nil? ? 1 : Integer.__index__(stride)
      raise ArgumentError, "step can't be negative" if stride < 0
      raise ArgumentError, "step can't be 0" if stride == 0
      return to_enum(:step, stride) if block.nil?
      index = 0
      each do |element|
        yield element if index % stride == 0
        index = index + 1
      end
      return self
    end
    raise ArgumentError, "step is required for non-numeric ranges" if stride.nil?
    return to_enum(:step, stride) if block.nil?
    raise ArgumentError, "#step iteration for beginless ranges is meaningless" if first.nil?
    term = first
    if last.nil?
      while true
        yield term
        term = term + stride
      end
    end
    # Which way the range runs, and one addition to see that the step runs
    # the same way; if it does not, there is nothing to yield. CRuby's
    # `range_step`.
    direction = first <=> last
    if (first <=> first + stride) == direction
      while true
        order = term <=> last
        break unless order == direction || (order == 0 && !exclude_end?)
        yield term
        break if order == 0
        term = term + stride
      end
    end
    self
  end

  def %(stride)
    first = self.begin
    if first.is_a?(Numeric) || (first.nil? && self.end.is_a?(Numeric))
      raise ArgumentError, "step can't be 0" if stride == 0
      return Enumerator::ArithmeticSequence.__make__(first, self.end, stride, exclude_end?, :%)
    end
    step(stride)
  end
end

# There is no walking from one point on a plane toward another in steps of a
# third. Here, because `step` is defined in this file.
class Complex
  undef_method :step
end
