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
    if cmp.nil?
      raise ArgumentError, "comparison of " + self.class.name + " with " + other.inspect + " failed"
    end
    cmp
  end
end
