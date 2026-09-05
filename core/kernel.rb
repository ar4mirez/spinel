# Kernel — the methods every object has.
#
# `send`, `raise`, `throw`, `catch`, `block_given?`, `proc`, `lambda`, `class`,
# `equal?`, `nil?`, `dup`, `freeze`, `frozen?`, `object_id` and `__write__` are
# primitives: each is dispatch, an allocation, a header bit, or a syscall.
# Everything below is Ruby, because Ruby can say it.
module Kernel
  def ===(other)
    self == other
  end

  def is_a?(mod)
    self.class.ancestors.include?(mod)
  end

  def kind_of?(mod)
    is_a?(mod)
  end

  def instance_of?(mod)
    self.class.equal?(mod)
  end

  def respond_to?(name, include_all = false)
    self.class.method_defined?(name, include_all)
  end

  def itself
    self
  end

  def then
    yield self
  end

  def yield_self
    yield self
  end

  def tap
    yield self
    self
  end

  def to_s
    "#<" + self.class.name + ">"
  end

  def inspect
    to_s
  end

  # `loop` stops on StopIteration rather than propagating it, which is what
  # makes `loop { enum.next }` end. Nothing raises it until Enumerator lands;
  # the rescue is the contract, not a placeholder.
  def loop
    begin
      while true
        yield
      end
    rescue StopIteration
      nil
    end
  end

  def puts(*lines)
    if lines.size == 0
      __write__("\n")
      return nil
    end
    i = 0
    while i < lines.size
      line = lines[i]
      text = line.nil? ? "" : line.to_s
      __write__(text)
      __write__("\n") unless text.end_with?("\n")
      i = i + 1
    end
    nil
  end

  def print(*parts)
    i = 0
    while i < parts.size
      __write__(parts[i].to_s)
      i = i + 1
    end
    nil
  end

  def p(*values)
    i = 0
    while i < values.size
      __write__(values[i].inspect)
      __write__("\n")
      i = i + 1
    end
    return nil if values.size == 0
    return values[0] if values.size == 1
    values
  end
end
