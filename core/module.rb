# Module — reflection over the class table.
#
# `name` and `ancestors` read tables only the VM has, so they are primitives.
# The predicates below are Ruby on top of them.
class Module
  def to_s
    n = name
    n.nil? ? "#<Module>" : n
  end

  def inspect
    to_s
  end

  def ===(object)
    object.is_a?(self)
  end

  def include?(mod)
    !equal?(mod) && ancestors.include?(mod)
  end

  def <(other)
    !equal?(other) && ancestors.include?(other)
  end

  def <=(other)
    equal?(other) || ancestors.include?(other)
  end

  def >(other)
    other < self
  end

  def >=(other)
    other <= self
  end
end
