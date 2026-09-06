# Comparable — five operators on one, which is the mixin's whole idea.
module Comparable
  def ==(other)
    cmp = (self <=> other)
    !cmp.nil? && cmp == 0
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

  def clamp(low, high)
    return low if self < low
    return high if self > high
    self
  end

  # `<=>` answering nil means the two are not comparable, and Ruby raises
  # rather than treating "no answer" as "not less than".
  def __cmp__(other)
    cmp = (self <=> other)
    __cmp_failed__(other) if cmp.nil?
    cmp
  end

  # CRuby's `rb_cmperr`, which `Numeric`'s relational operators raise too.
  def __cmp_failed__(other)
    raise ArgumentError,
          "comparison of " + self.class.to_s + " with " + __operand_name__(other) + " failed"
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
       other.is_a?(Symbol) || other.is_a?(Numeric)
      other.inspect
    else
      other.class.to_s
    end
  end
end
