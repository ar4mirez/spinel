# Class.
#
# `new` and `allocate` are primitives: both are allocation, and the shape is per
# class. `superclass` and `ancestors` read the class table.
class Class
  # `Class#initialize` runs when `Class.new` makes an anonymous class. Calling
  # it a second time would re-open a class that already has a superclass, and
  # Ruby refuses rather than silently rebasing the hierarchy.
  def initialize(superclass = nil)
    raise TypeError, "already initialized class"
  end
end
