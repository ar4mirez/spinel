# BasicObject — the root of the hierarchy, and deliberately almost empty.
#
# Every method defined here is one that a BasicObject subclass cannot avoid
# inheriting, which is the whole point of the class: a blank slate for proxies.
class BasicObject
  def initialize
  end

  def ==(other)
    equal?(other)
  end

  # Truthiness, by identity. `self == false` would ask the object's own `==`,
  # and an object that answers true to everything would then be falsy.
  def !
    equal?(false) || equal?(nil)
  end

  def !=(other)
    !(self == other)
  end
end
