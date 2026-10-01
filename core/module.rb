# Module — reflection over the class table.
#
# `name` and `ancestors` read tables only the VM has, so they are primitives.
# The predicates below are Ruby on top of them.
class Module
  # An anonymous module has no name, and Ruby answers `#<Class:0x...>` for one
  # — `self.class` rather than a literal, so a `Class` says `Class` and a
  # `Module` says `Module`. The address is `object_id` in hex, which is what
  # CRuby prints too.
  def to_s
    n = name
    return n unless n.nil?
    "#<" + self.class.name + ":0x" + object_id.to_s(16) + ">"
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

# `Module#name` is frozen, and the same String every call while the name stays
# the same — measured. The VM's name can change once (an anonymous module
# assigned to a constant), so the memo is checked against it.
class Module
  def name
    current = __name__
    return nil if current.nil?
    cached = @__name__
    return cached if !cached.nil? && cached == current
    return current.freeze if frozen?
    @__name__ = current.freeze
  end
end
