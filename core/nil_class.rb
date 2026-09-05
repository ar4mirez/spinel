# NilClass — the class of `nil`, which is an immediate with exactly one value.
class NilClass
  def to_s
    ""
  end

  def to_a
    []
  end

  def inspect
    "nil"
  end

  def nil?
    true
  end

  def &(other)
    false
  end

  def |(other)
    !!other
  end

  def ^(other)
    !!other
  end
end
