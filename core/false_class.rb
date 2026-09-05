# FalseClass — the class of `false`.
class FalseClass
  def to_s
    "false"
  end

  def inspect
    "false"
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
