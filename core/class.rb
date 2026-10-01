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

class Class
  # The hook `class C < P` fires on `P` (#28): a private no-op by default.
  def inherited(subclass)
  end

  private :inherited, :initialize
end

# A class is not a mixin: `Module`'s splice hooks and `module_function` are
# undefined on `Class`, as in CRuby.
class Class
  undef_method :append_features, :prepend_features, :extend_object, :module_function
end

class Class
  def attached_object
    raise TypeError, "'#{inspect}' is not a singleton class" unless singleton_class?
    __reflect_attached__(self)
  end
end
