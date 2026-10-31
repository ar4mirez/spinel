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

  # Read from the class table and never asked: nothing is sent to the object,
  # which may be a BasicObject, and an `is_a?` of its own has no say.
  def ===(object)
    __reflect_kind_of__(object, self)
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
  # Where a primitive that takes names sends a call holding one that is
  # neither a Symbol nor a String: each such name is converted with `to_str`,
  # once, and the call made again.
  #
  # ponytail: the second call is made from here, so an `attr_reader` given
  # such a name takes this frame's default visibility, which is public,
  # rather than the class body's.
  def __names_then__(method, count, *args, &block)
    converted = []
    args.each_with_index do |name, index|
      converted.push(index < count ? Module.__name_arg__(name) : name)
    end
    __send__(method, *converted, &block)
  end

  def self.__name_arg__(name)
    return name if name.is_a?(Symbol) || name.is_a?(String)
    converted = name.respond_to?(:to_str) ? name.to_str : nil
    return converted if converted.is_a?(String)
    raise TypeError, "#{name.inspect} is not a symbol nor a string"
  end

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
    # A capital, then word characters — which past ASCII is any character.
    unless symbol.to_s.match?(/\A[A-Z](?:[A-Za-z0-9_]|[^\x00-\x7F])*\z/)
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
    name = name.to_str if !name.is_a?(Symbol) && !name.is_a?(String) && name.respond_to?(:to_str)
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
    __const_missing__(symbol, :qualified, [])
  end

  def const_missing(name)
    full = equal?(Object) ? name.to_s : "#{inspect}::#{name}"
    raise NameError.new("uninitialized constant #{full}", name, receiver: self)
  end

  def const_defined?(name, inherit = true)
    name = name.to_str if !name.is_a?(Symbol) && !name.is_a?(String) && name.respond_to?(:to_str)
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
    symbol = __const_name__(name)
    return true unless __reflect_const_lookup__(self, symbol, inherit).nil?
    # A constant an autoload will define is defined, as CRuby has it.
    owners = inherit ? ancestors : [self]
    owners += [Object] if inherit && instance_of?(Module)
    owners.any? { |owner| !owner.__autoload_entry__(symbol).nil? }
  end

  def const_set(name, value)
    symbol = __const_name__(name)
    raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self) if frozen?
    redefined = !__reflect_const_lookup__(self, symbol, false).nil?
    __reflect_const_set__(self, symbol, value)
    if redefined
      __warning__("already initialized constant #{equal?(Object) ? symbol : "#{inspect}::#{symbol}"}")
    end
    __send__(:const_added, symbol)
    value
  end

  def constants(inherit = true)
    names = __reflect_const_names__(self, inherit)
    owners = inherit ? ancestors : [self]
    owners.each do |owner|
      entries = owner.instance_variable_get(:@__autoloads__)
      entries&.each_key { |name| names << name unless names.include?(name) }
    end
    names
  end

  # --- autoload (#39) ----------------------------------------------------------
  #
  # A name and the file that defines it, kept on the module until the first
  # reference to the name requires the file. The VM sends a reference it
  # cannot resolve to `__const_missing__`, which is where that happens.

  def autoload(name, path)
    symbol = Symbol === name ? name : IO.__str__(name).to_sym
    unless symbol.to_s.match?(/\A[A-Z]/)
      raise NameError.new("autoload must be constant name: #{symbol}", symbol)
    end
    path = path.respond_to?(:to_path) && !(String === path) ? path.to_path : IO.__str__(path)
    raise ArgumentError, "empty feature name" if path.empty?
    return nil unless __reflect_const_lookup__(self, symbol, false).nil?
    # A file already required, or being required now, is not autoloaded:
    # CRuby treats the constant as loaded.
    feature = begin
      Kernel.__find_feature__(path)
    rescue LoadError
      nil
    end
    return nil if feature && $LOADED_FEATURES.include?(feature)
    entries = instance_variable_get(:@__autoloads__) || instance_variable_set(:@__autoloads__, {})
    entries[symbol] = path.dup.freeze
    __send__(:const_added, symbol)
    nil
  end

  def autoload?(name, inherit = true)
    symbol = Symbol === name ? name : IO.__str__(name).to_sym
    owners = inherit ? ancestors : [self]
    owners.each do |owner|
      path = owner.__autoload_entry__(symbol)
      return path unless path.nil?
    end
    nil
  end

  def __autoload_entry__(symbol)
    entries = instance_variable_get(:@__autoloads__)
    entries && entries[symbol]
  end

  # Require what an autoload names, and answer the constant it defined.
  def __autoload__(symbol, path)
    require path
    @__autoloads__&.delete(symbol)
    found = __reflect_const_lookup__(self, symbol, false)
    return found[0] unless found.nil?
    nil
  end

  # A constant the VM did not find (#39): an autoload for the name, where the
  # reference would have looked — the lexical scopes, the ancestors, `Object`
  # — then the program's `const_missing`, then the `NameError` the VM would
  # have raised itself.
  def __const_missing__(name, how, nesting)
    owners = case how
             when :lexical then nesting + (nesting.first || self).ancestors + [Object]
             when :top then Object.ancestors
             else ancestors + (instance_of?(Module) ? [] : [])
             end
    owners = owners.uniq
    owners.each do |owner|
      path = owner.__autoload_entry__(name)
      next if path.nil?
      if owner.__autoload__(name, path).nil? && __reflect_const_lookup__(owner, name, false).nil?
        full = owner.equal?(Object) ? name.to_s : "#{owner.inspect}::#{name}"
        __warning__("Expected #{path} to define #{full} but it didn't", true)
      end
      # The original lookup again: what the file defined may be anywhere it
      # would have looked, an outer scope as well as the one it was declared in.
      owners.each do |scope|
        found = __reflect_const_lookup__(scope, name, false)
        return found[0] unless found.nil?
      end
    end
    location = method(:const_missing).source_location
    unless location.nil? || location[0].start_with?("<internal:")
      return const_missing(name)
    end
    __const_uninitialized__(self, name, how)
  end
  private :__const_missing__

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
      symbol = Module.__name_arg__(name).to_sym
      # After the name, before whether it is defined: CRuby's order, measured.
      Kernel.__check_frozen__(self)
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

  # `class_variable_defined?` is what checks the name is a class variable's,
  # in other words than this method's — measured. One held by an ancestor is
  # found and still not this module's to remove.
  def remove_class_variable(name)
    symbol = Module.__name_arg__(name).to_sym
    defined = begin
      class_variable_defined?(symbol)
    rescue NameError
      raise NameError.new("wrong class variable name #{symbol}", symbol, receiver: self)
    end
    Kernel.__check_frozen__(self)
    removed = __reflect_cvar_remove__(self, symbol)
    return removed[0] unless removed.nil?
    if defined
      raise NameError.new("cannot remove #{symbol} for #{self}", symbol, receiver: self)
    end
    raise NameError.new("class variable #{symbol} not defined for #{self}", symbol, receiver: self)
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

# What `new` on a subclass of `Module` runs (#28); `Module.new` itself is a
# primitive. The block is the module body, as it is for `Module.new`.
class Module
  def initialize(&block)
    module_exec(self, &block) if block
  end

  private :initialize
end

# `include`, `prepend` and `extend` are Ruby so that what they call is
# overridable (#28): every module is checked first, then, right to left so the
# first ends up nearest, `append_features` splices and `included` observes.
# The splice itself is the `__include__`/`__prepend__`/`__extend__` primitive.
# `while` over `size` and `[]` rather than `each`: the core library includes
# modules before `Array`'s Ruby half is loaded.
class Module
  def include(*modules)
    Module.__check_mixins__(modules, "include", self)
    i = modules.size
    while i > 0
      i -= 1
      modules[i].__send__(:append_features, self)
      modules[i].__send__(:included, self)
    end
    self
  end

  def prepend(*modules)
    Module.__check_mixins__(modules, "prepend", self)
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

  # CRuby asks for the singleton class before it checks frozen (#383): an
  # Integer, a Symbol or a frozen literal is refused with `TypeError`, and
  # `nil`, `true` and `false`, frozen as they are, extend their own class.
  def extend_object(object)
    __reflect_singleton_class__(object)
    unless nil.equal?(object) || true.equal?(object) || false.equal?(object)
      Kernel.__check_frozen__(object)
    end
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

  # `verb` names the caller, for the refinements' errors (#352): a refinement
  # can be neither mixed in nor have anything mixed into it. CRuby's order.
  def self.__check_mixins__(modules, verb, receiver = nil)
    if verb != "extend" && !__reflect_refined_class__(receiver).nil?
      Kernel.raise TypeError, "Refinement##{verb} has been removed"
    end
    Kernel.raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if modules.size == 0
    i = 0
    while i < modules.size
      mod = modules[i]
      unless __reflect_module_kind__(mod) == :module
        Kernel.raise TypeError, "wrong argument type #{mod.class} (expected Module)"
      end
      unless __reflect_refined_class__(mod).nil?
        Kernel.raise TypeError, verb == "extend" ? "Cannot extend object with refinement" : "Cannot #{verb} refinement"
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

class Module
  # A module is below what it inherits from or includes and above what
  # inherits from it; two unrelated ones, or a module and anything else, are
  # not comparable.
  def <=>(other)
    return 0 if equal?(other)
    return nil unless other.is_a?(Module)
    return -1 if self < other
    other < self ? 1 : nil
  end
end

# Refinements (#352). `refine` and `using` are primitives: both work on the
# caller's lexical scope, which only its frame holds. CRuby makes both
# private, and takes `refine` off `Class`.
class Module
  private :refine, :using

  def self.used_modules
    __used_modules__
  end
end

class Class
  undef_method :refine
end

# What `refine` makes: a module of its own class, which knows what it refines
# and which module's `refine` made it.
class Refinement < Module
  # A refinement is mixed into nothing: `include`, `prepend` and `extend`
  # refuse one before these could run, and CRuby removes them.
  undef_method :append_features, :prepend_features, :extend_object

  def target
    @__target__
  end

  # Copies the modules' own methods, written in Ruby, into this refinement,
  # where they see the refinements active here. Not their ancestors'.
  def import_methods(*modules)
    modules.each do |mod|
      unless __reflect_module_kind__(mod) == :module
        raise TypeError, "wrong argument type #{mod.class} (expected Module)"
      end
    end
    modules.each do |mod|
      if mod.ancestors.size > 1
        warn "#{mod} has ancestors, but Refinement#import_methods doesn't import their methods", uplevel: 0
      end
      name = __import_methods__(mod)
      raise ArgumentError, "Can't import method which is not defined with Ruby code: #{mod}##{name}" if name
    end
    self
  end
  private :import_methods

  # A refinement's methods continue into the class it refines, through the
  # superclass CRuby gives it, so its `instance_methods` lists that class's
  # too. Its `ancestors` do not: measured, `[refinement]`.
  def instance_methods(inherit = true)
    own = super(false)
    inherit ? own | @__target__.instance_methods : own
  end

  def inspect
    "#<refinement:#{@__target__.inspect}@#{@__defined_at__.inspect}>"
  end
  alias to_s inspect
end
