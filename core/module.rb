# Module — reflection over the class table.
#
# `name` and `ancestors` read tables only the VM has, so they are primitives.
# The predicates below are Ruby on top of them.
class Module
  # An anonymous module has no name, and Ruby answers `#<Class:0x...>` for one
  # — `self.class` rather than a literal, so a `Class` says `Class` and a
  # `Module` says `Module`. The address is `object_id` in hex, which is what
  # CRuby prints too.
  # A singleton class shows what it is attached to: a module by its own
  # `to_s`, any other object the way `Kernel#to_s` would. Measured.
  def to_s
    if singleton_class?
      attached = __reflect_attached__(self)
      shown = if __reflect_module_kind__(attached).nil?
        "#<" + attached.class.to_s + ":0x" + attached.__address__ + ">"
      else
        attached.to_s
      end
      return "#<Class:" + shown + ">"
    end
    n = name
    return n unless n.nil?
    ("#<" + self.class.name + ":0x" + __address__ + ">").__force_encoding__(0)
  end

  # A singleton class is frozen with the object it belongs to.
  def frozen?
    return true if super
    singleton_class? && __reflect_attached__(self).frozen?
  end

  def inspect
    to_s
  end

  def ===(object)
    object.is_a?(self)
  end

  def include?(mod)
    unless __reflect_module_kind__(mod) == :module
      raise TypeError, "wrong argument type #{mod.class} (expected Module)"
    end
    !equal?(mod) && ancestors.include?(mod)
  end

  # Ancestry as a partial order: true below, false above, nil when the two
  # are unrelated. Measured.
  def <(other)
    return false if equal?(other)
    self <= other
  end

  def <=(other)
    raise TypeError, "compared with non class/module" if __reflect_module_kind__(other).nil?
    return true if equal?(other) || ancestors.include?(other)
    return false if other.ancestors.include?(self)
    nil
  end

  def >(other)
    raise TypeError, "compared with non class/module" if __reflect_module_kind__(other).nil?
    other < self
  end

  def >=(other)
    raise TypeError, "compared with non class/module" if __reflect_module_kind__(other).nil?
    other <= self
  end
end

# `Module#name` is frozen, and the same String every call while the name stays
# the same — measured. The VM's name can change once (an anonymous module
# assigned to a constant), so the memo is checked against it.
class Module
  # Readers only, since Ruby 3 — except the obsolete `attr(name, true)`,
  # which still makes a writer too. Measured.
  def attr(*names)
    if names.size == 2 && (names[1] == true || names[1] == false)
      __warning__("optional boolean argument is obsoleted", true)
      return names[1] ? attr_accessor(names[0]) : attr_reader(names[0])
    end
    attr_reader(*names)
  end

  def name
    return nil if singleton_class?
    current = __name__
    return nil if current.nil?
    cached = @__name__
    return cached if !cached.nil? && cached == current
    return current.freeze if frozen?
    @__name__ = current.freeze
  end
end

# Reflection over the class table (#28). The tables are the VM's and are read
# through `__reflect_*__`; every name check, path walk and message is here.
# Measured against ruby 4.0.7.
class Module
  # A constant name as a Symbol: a Symbol, a String, or anything with
  # `to_str`, spelled as a constant — "wrong constant name" otherwise.
  def __const_name__(name)
    symbol = if name.is_a?(Symbol)
      name
    elsif name.is_a?(String)
      name.to_sym
    elsif name.respond_to?(:to_str)
      name.to_str.to_sym
    else
      raise TypeError, "no implicit conversion of #{name.nil? ? "nil" : name.class} into String"
    end
    unless symbol.to_s.match?(/\A[A-Z][A-Za-z0-9_]*\z/)
      raise NameError.new("wrong constant name #{symbol}", symbol)
    end
    symbol
  end

  # `"A::B"` as `["A", "B"]`, and whether it started at the top level.
  def __const_path__(text)
    top = text.start_with?("::")
    rest = top ? text[2, text.length - 2] : text
    parts = []
    until rest.nil? || rest.empty?
      at = rest.index("::")
      if at.nil?
        parts.push(rest)
        rest = nil
      else
        parts.push(rest[0, at])
        rest = rest[at + 2, rest.length - at - 2]
      end
    end
    [top, parts]
  end

  def __const_path?(name)
    text = name.is_a?(Symbol) ? nil : (name.is_a?(String) ? name : (name.respond_to?(:to_str) ? name.to_str : nil))
    !text.nil? && text.include?("::") ? text : nil
  end

  def const_get(name, inherit = true)
    path = __const_path?(name)
    unless path.nil?
      top, parts = __const_path__(path)
      raise NameError.new("wrong constant name #{path}", path.to_sym) if parts.empty?
      base = top ? Object : self
      parts.each_with_index do |part, i|
        unless base.is_a?(Module)
          raise TypeError, "#{base.inspect} does not refer to class/module"
        end
        if i == 0
          base = base.const_get(part, inherit)
        else
          # Past the first segment the walk is a qualified one, `A::B`'s:
          # it does not fall back to `Object`. Measured.
          symbol = __const_name__(part)
          found = __reflect_const_lookup__(base, symbol, inherit ? :qualified : false)
          base = found.nil? ? base.__send__(:const_missing, symbol) : found[0]
        end
      end
      return base
    end
    symbol = __const_name__(name)
    found = __reflect_const_lookup__(self, symbol, inherit)
    unless found.nil?
      __const_deprecation__(symbol)
      return found[0]
    end
    const_missing(symbol)
  end

  def const_missing(name)
    full = equal?(Object) ? name.to_s : "#{inspect}::#{name}"
    raise NameError.new("uninitialized constant #{full}", name, receiver: self)
  end

  def const_defined?(name, inherit = true)
    path = __const_path?(name)
    unless path.nil?
      top, parts = __const_path__(path)
      base = top ? Object : self
      parts.each do |part|
        return false unless base.is_a?(Module) && base.const_defined?(part, inherit)
        base = base.const_get(part, inherit)
      end
      return true
    end
    !__reflect_const_lookup__(self, __const_name__(name), inherit).nil?
  end

  def const_set(name, value)
    symbol = __const_name__(name)
    raise FrozenError, "can't modify frozen #{self.class}: #{inspect}" if frozen?
    redefined = !__reflect_const_lookup__(self, symbol, false).nil?
    __reflect_const_set__(self, symbol, value)
    if redefined
      __warning__("already initialized constant #{equal?(Object) ? symbol : "#{inspect}::#{symbol}"}")
    end
    __send__(:const_added, symbol)
    value
  end

  def constants(inherit = true)
    __reflect_const_names__(self, inherit)
  end

  def remove_const(name)
    symbol = __const_name__(name)
    __const_deprecation__(symbol)
    removed = __reflect_const_remove__(self, symbol)
    if removed.nil?
      raise NameError.new("constant #{inspect}::#{symbol} not defined", symbol)
    end
    removed[0]
  end
  private :remove_const

  # Say so if `symbol`, as `const_get` would find it from here, was marked by
  # `deprecate_constant`.
  def __const_deprecation__(symbol)
    holder = __reflect_const_deprecated__(self, symbol, false)
    return if holder.nil?
    __warning__("constant #{holder.inspect}::#{symbol} is deprecated", false, :deprecated)
  end
  private :__const_deprecation__

  def public_constant(*names)
    names.each do |name|
      symbol = __const_name__(name)
      unless __reflect_const_public__(self, symbol)
        raise NameError.new("constant #{inspect}::#{symbol} not defined", symbol)
      end
    end
    self
  end

  # Deprecation is a warning, and warnings need `$stderr` (#39); the constant
  # itself is unchanged, which is all ruby/spec can observe without one.
  # Deprecation warnings are not shown, but an unknown name is still an error.
  def deprecate_constant(*names)
    names.each do |name|
      symbol = __const_name__(name)
      if __reflect_const_lookup__(self, symbol, false).nil?
        raise NameError.new("constant #{inspect}::#{symbol} not defined", symbol)
      end
      __reflect_const_deprecated__(self, symbol, true)
    end
    self
  end

  def instance_methods(inherit = true)
    __reflect_method_names__(self, inherit, 0)
  end

  def public_instance_methods(inherit = true)
    __reflect_method_names__(self, inherit, 1)
  end

  def protected_instance_methods(inherit = true)
    __reflect_method_names__(self, inherit, 2)
  end

  def private_instance_methods(inherit = true)
    __reflect_method_names__(self, inherit, 3)
  end

  def remove_method(*names)
    names.each do |name|
      symbol = name.is_a?(Symbol) ? name : name.to_str.to_sym
      unless __reflect_remove_method__(self, symbol)
        raise NameError.new("method '#{symbol}' not defined in #{inspect}", symbol, receiver: self)
      end
      # The hook (#28): on the object behind a singleton class, on the
      # module otherwise.
      if singleton_class?
        __reflect_attached__(self).__send__(:singleton_method_removed, symbol)
      else
        __send__(:method_removed, symbol)
      end
    end
    self
  end

  def singleton_class?
    __reflect_is_singleton__(self)
  end
end

# The definition hooks' defaults (#28): private no-ops the VM skips. A
# program's own override is what fires.
class Module
  def method_added(name)
  end

  def method_removed(name)
  end

  def method_undefined(name)
  end

  def included(base)
  end

  def extended(base)
  end

  def prepended(base)
  end

  def const_added(name)
  end

  private :method_added, :method_removed, :method_undefined, :included,
          :extended, :prepended, :const_added
end

# `include`, `prepend` and `extend` are Ruby so that what they call is
# overridable (#28): every module is checked first, then, right to left so the
# first ends up nearest, `append_features` splices and `included` observes.
# The splice itself is the `__include__`/`__prepend__`/`__extend__` primitive.
# `while` over `size` and `[]` rather than `each`: the core library includes
# modules before `Array`'s Ruby half is loaded.
class Module
  def include(*modules)
    Module.__check_mixins__(modules)
    i = modules.size
    while i > 0
      i -= 1
      modules[i].__send__(:append_features, self)
      modules[i].__send__(:included, self)
    end
    self
  end

  def prepend(*modules)
    Module.__check_mixins__(modules)
    i = modules.size
    while i > 0
      i -= 1
      modules[i].__send__(:prepend_features, self)
      modules[i].__send__(:prepended, self)
    end
    self
  end

  def append_features(target)
    Module.__check_target__(target)
    Kernel.__check_frozen__(target)
    target.__include__(self)
    self
  end

  def prepend_features(target)
    Module.__check_target__(target)
    Kernel.__check_frozen__(target)
    target.__prepend__(self)
    self
  end

  def extend_object(object)
    Kernel.__check_frozen__(object)
    object.__extend__(self)
    object
  end

  private :append_features, :prepend_features, :extend_object,
          :module_function, :private, :protected, :public

  # `Module.constants` with no arguments is every constant reachable from
  # the top level — `Object`'s, through its ancestors.
  def self.constants(*args)
    return super unless args.empty?
    Object.constants
  end

  def self.__check_mixins__(modules)
    Kernel.raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if modules.size == 0
    i = 0
    while i < modules.size
      mod = modules[i]
      unless __reflect_module_kind__(mod) == :module
        Kernel.raise TypeError, "wrong argument type #{mod.class} (expected Module)"
      end
      i += 1
    end
  end

  def self.__check_target__(target)
    if __reflect_module_kind__(target).nil?
      Kernel.raise TypeError, "wrong argument type #{target.class} (expected Class)"
    end
  end
end

# `BasicObject::BasicObject`, so a blank slate can still name its own root:
# CRuby defines it, and `BasicObject.constants` lists it. The primitive, as
# `Array#each` is not loaded yet for `const_set`'s name check.
__reflect_const_set__(BasicObject, :BasicObject, BasicObject)

class Module
  # Visibility on the singleton: names, or one Array of them. Answers self.
  def private_class_method(*names)
    names = names[0] if names.size == 1 && names[0].is_a?(Array)
    singleton_class.__send__(:private, *names)
    self
  end

  def public_class_method(*names)
    names = names[0] if names.size == 1 && names[0].is_a?(Array)
    singleton_class.__send__(:public, *names)
    self
  end
end

# `Module.nesting` is the primitive itself, aliased rather than wrapped: a
# Ruby wrapper's frame would answer its own scope instead of the caller's.
class << Module
  alias nesting __module_nesting__
end
