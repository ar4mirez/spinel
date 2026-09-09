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

  # `nil` is equal to itself and not comparable to anything else — 0 or nil,
  # never an error. Reached through `Array#<=>` whenever a nil sits in a pair.
  def <=>(other)
    other.nil? ? 0 : nil
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
