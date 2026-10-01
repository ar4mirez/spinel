# BasicObject — the root of the hierarchy, and deliberately almost empty.
#
# Every method defined here is one that a BasicObject subclass cannot avoid
# inheriting, which is the whole point of the class: a blank slate for proxies.
class BasicObject
  def initialize
  end

  # Identity, and not through `equal?`: measured, a class that overrides
  # `equal?` to answer false still has `o == o` true.
  def ==(other)
    __identical__(other)
  end

  # Truthiness, by identity. `self == false` would ask the object's own `==`,
  # and an object that answers true to everything would then be falsy.
  def !
    __identical__(false) || __identical__(nil)
  end

  def !=(other)
    !(self == other)
  end
end

# The hooks and defaults every object has (#28). Private, as in CRuby, and
# no-ops: the VM fires a hook only when the program overrides one of these.
class BasicObject
  def singleton_method_added(name)
  end

  def singleton_method_removed(name)
  end

  def singleton_method_undefined(name)
  end

  # Called directly — `obj.send(:method_missing, :x)` — it raises what a real
  # miss would. The VM never calls this one: a miss with no `method_missing`
  # of the program's own raises from dispatch.
  def method_missing(*args)
    ::Kernel.raise ::ArgumentError, "no method name given" if args.empty?
    name = args[0]
    unless name.is_a?(::Symbol)
      ::Kernel.raise ::ArgumentError, "method name must be a Symbol but #{name.class} is given"
    end
    __raise_no_method__(name, args.__take__(1, args.size - 1))
  end

  private :initialize, :method_missing, :singleton_method_added,
          :singleton_method_removed, :singleton_method_undefined
end
