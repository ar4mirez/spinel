# Comparable — five operators on one, which is the mixin's whole idea.
module Comparable
  # Identity first, and a nil `<=>` means "not equal" rather than an error.
  #
  # `<=>` may itself call `==` — `Kernel#<=>` does, so a class that includes
  # Comparable without defining `<=>`, or whose `<=>` calls `super`, would
  # recurse forever. CRuby's `rb_exec_recursive_paired_outer` stops that: once
  # a pair meets itself, the outermost comparison of it answers false.
  def ==(other)
    return true if equal?(other)
    comparing = (Thread.current.__recursive__[:comparable] ||= [])
    throw :__spinel_comparable_recursion__ if comparing.any? { |a, b| a.equal?(self) && b.equal?(other) }
    outermost = comparing.empty?
    comparing.push([self, other])
    begin
      return __equal_cmp__(other) unless outermost
      finished = false
      equal = catch(:__spinel_comparable_recursion__) do
        answer = __equal_cmp__(other)
        finished = true
        answer
      end
      finished && equal
    ensure
      comparing.pop
      Thread.current.__recursive_done__(:comparable) if outermost
    end
  end

  def <(other)
    __cmp__(other) < 0
  end

  def <=(other)
    __cmp__(other) <= 0
  end

  def >(other)
    __cmp__(other) > 0
  end

  def >=(other)
    __cmp__(other) >= 0
  end

  def between?(low, high)
    self >= low && self <= high
  end

  # CRuby's `cmp_clamp`: two bounds or one Range (or, as `rb_range_values`
  # takes it, anything answering `begin`, `end` and `exclude_end?`), and a nil bound (or an
  # endless or beginless Range) leaves that side open. The bounds are checked
  # against each other before `self` is compared with either, so
  # `1.clamp(3, 1)` raises whatever `self` is.
  def clamp(*args)
    if args.size == 1
      range = args[0]
      unless range.is_a?(Range) ||
             (!range.is_a?(Enumerator::ArithmeticSequence) && range.respond_to?(:begin) &&
              range.respond_to?(:end) && range.respond_to?(:exclude_end?))
        raise TypeError, "wrong argument type " + __builtin_class_name__(range) + " (expected Range)"
      end
      low = range.begin
      high = range.end
      if !high.nil? && range.exclude_end?
        raise ArgumentError, "cannot clamp with an exclusive range"
      end
    elsif args.size == 2
      low, high = args
    else
      raise ArgumentError, "wrong number of arguments (given " + args.size.to_s + ", expected 1..2)"
    end
    if !low.nil? && !high.nil? && __cmpint__(low <=> high, high, low) > 0
      raise ArgumentError, "min argument must be less than or equal to max argument"
    end
    unless low.nil?
      c = __cmp__(low)
      return self if c == 0
      return low if c < 0
    end
    return high if !high.nil? && __cmp__(high) > 0
    self
  end

  # `<=>` answering nil means the two are not comparable, and Ruby raises
  # rather than treating "no answer" as "not less than".
  def __cmp__(other)
    __cmpint__(self <=> other, other)
  end

  # `==`'s comparison: nil is "not equal", anything else is read as `rb_cmpint`
  # reads it, so an answer that is not a number raises.
  def __equal_cmp__(other)
    cmp = (self <=> other)
    !cmp.nil? && __cmpint__(cmp, other) == 0
  end

  # CRuby's `rb_cmpint`: what `<=>` answered, as -1, 0 or 1. An Integer is
  # taken as it is; anything else by asking it how it compares with 0, which
  # is where a String answer raises. `receiver` is whoever answered, which
  # `clamp` needs when it compares its bounds with each other.
  def __cmpint__(cmp, other, receiver = self)
    __cmp_failed__(other, receiver) if cmp.nil?
    return cmp if cmp.is_a?(Integer)
    return 1 if cmp > 0
    return -1 if cmp < 0
    0
  end

  # How `rb_builtin_class_name` names an argument of the wrong type.
  def __builtin_class_name__(value)
    return "nil" if value.nil?
    return "true" if true.equal?(value)
    return "false" if false.equal?(value)
    value.class.to_s
  end

  # CRuby's `rb_cmperr`, which `Numeric`'s relational operators raise too.
  def __cmp_failed__(other, receiver = self)
    raise ArgumentError,
          "comparison of " + receiver.class.to_s + " with " + __operand_name__(other) + " failed"
  end

  # How Ruby names the other operand when it complains about it.
  #
  # An immediate is named by its `inspect` and everything else by its class:
  # "comparison of Float with nil failed", but "comparison of Float with Object
  # failed" — not `#<Object:0x...>`. Measured; it is `rb_cmperr`'s
  # `SPECIAL_CONST_P` test, and the same rule names the operand `Float()`
  # refuses to convert.
  def __operand_name__(other)
    if other.nil? || true.equal?(other) || false.equal?(other) ||
       other.is_a?(Symbol) || other.is_a?(Integer) || other.is_a?(Float)
      other.inspect
    else
      other.class.to_s
    end
  end
end
