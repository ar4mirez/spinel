# TrueClass — the class of `true`.
class TrueClass
  def to_s
    "true"
  end

  def inspect
    "true"
  end

  def &(other)
    !!other
  end

  def |(other)
    true
  end

  def ^(other)
    !other
  end
end
