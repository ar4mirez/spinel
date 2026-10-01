# FalseClass — the class of `false`.
class FalseClass
  # One frozen String, the same object every time: measured, and
  # `false/to_s_spec.rb` checks both.
  def to_s
    FalseClass.__to_s__
  end

  def self.__to_s__
    @__to_s__ ||= "false".freeze
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
