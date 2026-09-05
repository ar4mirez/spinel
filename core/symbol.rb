# Symbol.
#
# `to_s`, `name`, `length` and `size` are primitives: they read the shared
# symbol table, which is the one table no heap owns.
class Symbol
  def to_sym
    self
  end

  def inspect
    ":" + to_s
  end

  def empty?
    length == 0
  end

  def <=>(other)
    return nil unless other.is_a?(Symbol)
    to_s <=> other.to_s
  end
end
