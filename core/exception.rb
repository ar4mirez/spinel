# Exception.
#
# `message`, `to_s` and `backtrace` are primitives, because the message is in a
# slot the VM writes when it raises. `full_message` and `detailed_message` wait
# for backtraces, which PRD 0012 named as a non-goal.
class Exception
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
