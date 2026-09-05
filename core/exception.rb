# Exception.
#
# The VM writes `@message` when it raises, so everything here is ordinary Ruby
# reading an ordinary instance variable. It was two primitives over two fixed
# slots until #151 gave objects a shape to hold ivars in.
#
# `backtrace` is always nil: a real one needs source positions the compiler does
# not record, and `[]` would be a plausible-but-wrong answer rather than an
# absent one. `full_message` and `detailed_message` wait on the same thing,
# which PRD 0012 named as a non-goal.
class Exception
  def message
    @message
  end

  def to_s
    @message
  end

  def backtrace
    @backtrace
  end

  def inspect
    text = message
    text.empty? ? self.class.name : "#<" + self.class.name + ": " + text + ">"
  end

  def exception(text = nil)
    return self if text.nil?
    self.class.new(text)
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Exception)
    self.class.equal?(other.class) && message == other.message
  end
end
