# Class.
#
# `new` and `allocate` are primitives: both are allocation, and the shape is per
# class. `superclass` and `ancestors` read the class table.
class Class
  # `Class#initialize` gives a class `Class.allocate` made its superclass, as
  # CRuby's `rb_class_initialize` does (#28); `Class.new` is a primitive that
  # never calls it. Calling it on a class that already has a superclass would
  # rebase the hierarchy, and Ruby refuses.
  def initialize(superclass = Object, &block)
    raise TypeError, "already initialized class" unless __reflect_uninitialized__(self)
    unless __reflect_module_kind__(superclass) == :class
      raise TypeError, "superclass must be an instance of Class (given an instance of #{superclass.class})"
    end
    raise TypeError, "can't make subclass of singleton class" if superclass.singleton_class?
    raise TypeError, "can't make subclass of Class" if superclass.equal?(Class)
    raise TypeError, "can't inherit uninitialized class" if __reflect_uninitialized__(superclass)
    __reflect_inherit__(self, superclass)
    superclass.__send__(:inherited, self)
    module_exec(self, &block) if block
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
