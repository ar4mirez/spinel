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

  # `eql?` is `hash`'s partner: two objects that are `eql?` must have the same
  # `hash`, and a `Hash` looks a key up by both. The default is identity, and
  # the classes whose `==` is by value override it below — with a class check,
  # because `1.eql?(1.0)` is false where `1 == 1.0` is true.
  def eql?(other)
    equal?(other)
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

  # `self.class.to_s`, not `.name`: an instance of an anonymous class has a
  # class whose `name` is nil, and `"#<" + nil` is a TypeError.
  #
  # ponytail: Ruby puts the address in here too — `#<Foo:0x...>`. #15 left that
  # out rather than invent one, and this slice does not change it.
  def to_s
    "#<" + self.class.to_s + ">"
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
