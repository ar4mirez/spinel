# TrueClass — the class of `true`.
class TrueClass
  # One frozen String, the same object every time: measured, and
  # `true/to_s_spec.rb` checks both.
  def to_s
    TrueClass.__to_s__
  end

  def self.__to_s__
    @__to_s__ ||= "true".freeze
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
