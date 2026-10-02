# NilClass — the class of `nil`, which is an immediate with exactly one value.
class NilClass
  # One frozen String, the same object every time: measured, and
  # `nil/to_s_spec.rb` checks both.
  def to_s
    NilClass.__to_s__
  end

  def self.__to_s__
    @__to_s__ ||= "".freeze
  end

  def to_a
    []
  end

  def to_h
    {}
  end

  def to_i
    0
  end

  def to_f
    0.0
  end

  def =~(other)
    nil
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
