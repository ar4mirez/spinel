# Kernel — the methods every object has.
#
# `send`, `raise`, `throw`, `catch`, `block_given?`, `proc`, `lambda`, `class`,
# `equal?`, `nil?`, `__dup__` (the copy under `dup`), `freeze`, `frozen?`,
# `object_id` and `__write__` are primitives: each is dispatch, an allocation, a header bit, or a syscall.
# Everything below is Ruby, because Ruby can say it.
module Kernel
  # The same object first, whatever `==` and `equal?` say — CRuby's
  # `rb_equal` — and `==` otherwise. Measured with both overridden to false.
  def ===(other)
    __id__ == other.__id__ || self == other ? true : false
  end

  # `class or module required` rather than `false`: asking whether an object is
  # a kind of `1` is a mistake in the caller, and Ruby says so. Measured.
  #
  # The check runs only when the answer would be false — a `mod` that is in the
  # ancestry is a module already — so the common path costs what it always did.
  #
  # The ancestry is the object's real one, singleton class first (#202): a
  # module it was `extend`ed with counts, and an overridden `class` does not.
  def is_a?(mod)
    return true if __reflect_class_of__(self).ancestors.include?(mod)
    raise TypeError, "class or module required" if __reflect_module_kind__(mod).nil?
    false
  end

  def kind_of?(mod)
    is_a?(mod)
  end

  def instance_of?(mod)
    return true if self.class.equal?(mod)
    raise TypeError, "class or module required" unless mod.class.ancestors.include?(Module)
    false
  end

  # `eql?` is `hash`'s partner: two objects that are `eql?` must have the same
  # `hash`, and a `Hash` looks a key up by both. The default is identity, and
  # the classes whose `==` is by value override it below — with a class check,
  # because `1.eql?(1.0)` is false where `1 == 1.0` is true.
  def eql?(other)
    equal?(other)
  end

  def itself
    self
  end

  def then
    yield self
  end

  def yield_self
    yield self
  end

  def tap
    yield self
    self
  end

  # `self.class.to_s`, not `.name`: an instance of an anonymous class has a
  # class whose `name` is nil, and `"#<" + nil` is a TypeError. The address is
  # `object_id` in hex, padded to sixteen digits as CRuby's is — the same
  # number `Module#to_s` prints for an anonymous class.
  # BINARY, as CRuby's `rb_any_to_s` answers. Measured.
  def to_s
    ("#<" + self.class.to_s + ":0x" + __address__ + ">").__force_encoding__(0)
  end

  # `to_s` plus the instance variables, measured on ruby 4.0.7:
  # `#<Foo:0x... @a=1, @b="x">`. It never calls `to_s`, which a subclass may
  # have overridden, and an object met again while inspecting itself prints as
  # `#<Foo:0x... ...>`. Ruby 4.0 lets a class choose which variables to show
  # with a private `instance_variables_to_inspect`.
  def inspect
    head = "#<" + self.class.to_s + ":0x" + __address__
    names = if respond_to?(:instance_variables_to_inspect, true)
      chosen = __send__(:instance_variables_to_inspect)
      unless chosen.nil? || chosen.is_a?(Array)
        raise TypeError, "Expected #instance_variables_to_inspect to return an Array or nil, " \
                         "but it returned #{chosen.class}"
      end
      # Names it lists that the object does not have are left out. Measured.
      if chosen.nil?
        instance_variables.reject { |name| name.to_s.start_with?("@__") && name.to_s.end_with?("__") }
      else
        chosen.select { |name| instance_variable_defined?(name) }
      end
    else
      instance_variables.reject { |name| name.to_s.start_with?("@__") && name.to_s.end_with?("__") }
    end
    return (head + ">").__force_encoding__(0) if names.empty?
    inspecting = Kernel.__inspecting__
    return head + " ...>" if inspecting.any? { |seen| seen.equal?(self) }
    inspecting.push(self)
    begin
      parts = names.map { |name| name.to_s + "=" + instance_variable_get(name).inspect }
    ensure
      inspecting.pop
    end
    head + " " + parts.join(", ") + ">"
  end

  def __address__
    hex = object_id.to_s(16)
    hex = "0" + hex while hex.length < 16
    hex
  end

  # The objects `inspect` is part-way through, innermost last: what stops an
  # object that holds itself from inspecting forever. Per heap, on `Kernel`.
  def self.__inspecting__
    @__inspecting__ ||= []
  end

  # Run the block unless `object` is already being inspected further out,
  # and answer `placeholder` if it is: `[1, [...]]`, `{x: {...}}`.
  def self.__inspect_guard__(object, placeholder)
    inspecting = __inspecting__
    return placeholder if inspecting.any? { |seen| seen.equal?(object) }
    inspecting.push(object)
    begin
      yield
    ensure
      inspecting.pop
    end
  end

  # `hash` for a structure that may contain itself (#22), which is CRuby's
  # `rb_exec_recursive_outer`: the block computes the real digest, and if any
  # object meets itself anywhere below, the *outermost* call answers
  # `recursive` instead — a value that depends only on the outer object's kind
  # and size. That is what makes `rec = []; rec << rec` hash like `[rec]` and
  # `[[rec]]`, which `eql?` says it must. Measured on ruby 4.0.7.
  def __recursive_hash__(recursive)
    stack = Kernel.__hashing__
    throw :__spinel_hash_recursion__ if stack.any? { |seen| seen.equal?(self) }
    outermost = stack.empty?
    stack.push(self)
    begin
      return yield unless outermost
      finished = false
      value = catch(:__spinel_hash_recursion__) do
        digest = yield
        finished = true
        digest
      end
      finished ? value : recursive
    ensure
      stack.pop
    end
  end

  # The objects `hash` is part-way through, innermost last. Per heap.
  def self.__hashing__
    @__hashing__ ||= []
  end

  # An element's `hash`, through `to_int` when it answers something else — the
  # conversion CRuby's `Array#hash` makes, and `array/hash_spec.rb` pins.
  def __element_hash__(value)
    code = value.hash
    code.is_a?(Integer) ? code : code.to_int
  end

  # `loop` stops on StopIteration rather than propagating it, which is what
  # makes `loop { enum.next }` end. Nothing raises it until Enumerator lands;
  # the rescue is the contract, not a placeholder.
  def loop
    begin
      while true
        yield
      end
    rescue StopIteration => stop
      # The finished iterator's own return value, measured: `loop { e.next }`
      # answers what `e`'s `each` returned.
      stop.result
    end
  end

  # -- pattern matching (#165) ------------------------------------------
  #
  # The compiler lowers a pattern to tests and jumps, and calls these for the
  # parts that are protocol rather than control flow. Ruby rather than
  # instructions because every one of them is a send and a comparison, which
  # `docs/engine.md` says belongs in `core/*.rb`; the compiler emitting them
  # inline would be ten instructions apiece for no gain.
  #
  # Private, and named so no program can mean them.

  # `subject.deconstruct` for an array pattern, or `nil` when the object has
  # none — which is a failed match, not an error. A `deconstruct` that answers
  # something other than an Array *is* an error: measured,
  # "deconstruct must return Array".
  def __pattern_deconstruct__(subject)
    return nil unless subject.respond_to?(:deconstruct)
    parts = subject.deconstruct
    raise TypeError, "deconstruct must return Array" unless parts.is_a?(Array)
    parts
  end

  # The same for a hash pattern. `keys` is the list of keys the pattern names,
  # or `nil` when it has a `**rest` and so may want all of them — which is what
  # CRuby passes, measured by giving an object a `deconstruct_keys` that
  # records its argument.
  def __pattern_deconstruct_keys__(subject, keys)
    return nil unless subject.respond_to?(:deconstruct_keys)
    pairs = subject.deconstruct_keys(keys)
    raise TypeError, "deconstruct_keys must return Hash" unless pairs.is_a?(Hash)
    pairs
  end

  # What `**rest` binds: everything the pattern did not name.
  def __pattern_rest__(pairs, named)
    out = {}
    pairs.each_pair { |key, value| out[key] = value unless named.include?(key) }
    out
  end

  # `case`/`in` with no `else`, and the `=>` form, when nothing matched.
  def __pattern_fail__(subject)
    raise NoMatchingPatternError, subject.inspect
  end

  # The `=>` form again, for a key the subject does not have. A separate class
  # in Ruby, and a separate message.
  def __pattern_key_fail__(subject, key)
    raise NoMatchingPatternKeyError, subject.inspect + ": key not found: " + key.inspect
  end

  # `to_enum(:each_slice, 2)` remembers a call so it can be made later, which is
  # what every Enumerable method without a block returns. `enum_for` is the same
  # method under Ruby's other name for it.
  def to_enum(method = :each, *arguments, &size)
    Enumerator.__for__(self, method, arguments, size)
  end

  def enum_for(method = :each, *arguments, &size)
    Enumerator.__for__(self, method, arguments, size)
  end

  # The module functions (#161). Each becomes a private instance method and a
  # public singleton method, which is why `defined?(Object.print)` is nil while
  # `defined?(Kernel.puts)` is "method" — both measured on ruby 4.0.6.
  # `Kernel.private_instance_methods(false)` is where the list comes from; the
  # primitive half of it — `raise`, `proc`, `lambda`, `catch`, `throw`,
  # `block_given?` — is marked in `interp.rs`, beside where those are defined.
  module_function :loop
end

# Object-side reflection (#28), over the same `__reflect_*__` primitives as
# `core/module.rb`.
module Kernel
  def singleton_class
    __reflect_singleton_class__(self)
  end

  # Public and protected methods; `methods(false)` is the singleton ones.
  def methods(regular = true)
    return singleton_methods(false) unless regular
    __reflect_method_names__(__reflect_class_of__(self), true, 0)
  end

  def public_methods(all = true)
    all ? __reflect_method_names__(__reflect_class_of__(self), true, 1) : __own_method_names__(1)
  end

  def protected_methods(all = true)
    all ? __reflect_method_names__(__reflect_class_of__(self), true, 2) : __own_method_names__(2)
  end

  def private_methods(all = true)
    all ? __reflect_method_names__(__reflect_class_of__(self), true, 3) : __own_method_names__(3)
  end

  # Without `all`, the singleton class's methods and then the object's own
  # class's, but no ancestor's: `Class.private_methods(false)` lists
  # `Class#initialize`. Measured.
  def __own_method_names__(which)
    klass = __reflect_class_of__(self)
    names = __reflect_method_names__(klass, false, which)
    if __reflect_is_singleton__(klass)
      __reflect_method_names__(self.class, false, which).each { |name| names.push(name) unless names.include?(name) }
    end
    names
  end

  # The methods on this object's own singleton class — and with `all`, those
  # of modules it was extended with and, for a class, its superclasses'
  # singleton methods. Measured.
  def singleton_methods(all = true)
    klass = __reflect_class_of__(self)
    return [] unless __reflect_is_singleton__(klass)
    names = __reflect_method_names__(klass, false, 0)
    return names unless all
    klass.ancestors.each do |ancestor|
      next if ancestor.equal?(klass)
      break unless ancestor.instance_of?(Module) || __reflect_is_singleton__(ancestor)
      __reflect_method_names__(ancestor, false, 0).each { |name| names.push(name) unless names.include?(name) }
    end
    names
  end
end

# The copy hooks (#28), private as in CRuby. `initialize_dup` and
# `initialize_clone` call `initialize_copy`, which does nothing by default.
module Kernel
  def initialize_copy(original)
    return self if equal?(original)
    Kernel.__check_frozen__(self)
    unless original.class.equal?(self.class)
      raise TypeError, "initialize_copy should take same class object"
    end
    self
  end

  # The `FrozenError` every mutator raises: the class, then `inspect`.
  def self.__check_frozen__(object)
    return unless object.frozen?
    raise FrozenError.new("can't modify frozen #{object.class}: #{object.inspect}", receiver: object)
  end

  # The copy is the primitive's; what it means for the class is
  # `initialize_copy`'s, which a program overrides — mspec's `ContextState`
  # drops its cached hook lists there, and a copy that skipped it ran one
  # shared group's hooks for the next.
  def dup
    copy = __dup__
    copy.__send__(:initialize_dup, self) unless copy.equal?(self)
    copy
  end
  
  def initialize_dup(original)
    initialize_copy(original)
  end

  def initialize_clone(original, freeze: nil)
    initialize_copy(original)
  end

  # What a program's own `respond_to_missing?` overrides; the VM only asks one
  # that is not this.
  def respond_to_missing?(name, include_all)
    false
  end

  private :initialize_copy, :initialize_dup, :initialize_clone, :respond_to_missing?
end

# `extend` is Ruby so that `extend_object` and `extended` run (#28); `Kernel`,
# not `Object`, so a `BasicObject` subclass that includes `Kernel` gets it.
module Kernel
  def extend(*modules)
    Module.__check_mixins__(modules)
    i = modules.size
    while i > 0
      i -= 1
      modules[i].__send__(:extend_object, self)
      modules[i].__send__(:extended, self)
    end
    self
  end
end

# The conversion functions (#28). Each tries the implicit conversion first and
# the explicit one second, privately — `respond_to?(name, true)` and
# `__send__` — because Ruby's do too. Every message measured on ruby 4.0.7.
module Kernel
  def Array(object)
    return object if Array === object
    return [] if object.nil?
    [:to_ary, :to_a].each do |name|
      next unless object.respond_to?(name, true)
      converted = object.__send__(name)
      return converted if Array === converted
      next if converted.nil?
      raise TypeError,
            "can't convert #{object.class} to Array (#{object.class}##{name} gives #{converted.class})"
    end
    [object]
  end

  def Hash(object)
    return {} if object.nil?
    return object if Hash === object
    if object.respond_to?(:to_hash, true)
      converted = object.__send__(:to_hash)
      return converted if Hash === converted
      raise TypeError,
            "can't convert #{object.class} to Hash (#{object.class}#to_hash gives #{converted.class})"
    end
    return {} if Array === object && object.empty?
    raise TypeError, "can't convert #{object.class} into Hash"
  end

  def Integer(object, base = nil, exception: true)
    unless exception == true || exception == false
      raise ArgumentError, "expected true or false as exception: #{exception.inspect}"
    end
    return Kernel.__integer__(object, base) if exception
    begin
      Kernel.__integer__(object, base)
    rescue ArgumentError, TypeError, FloatDomainError
      nil
    end
  end

  def self.__integer__(object, base)
    unless base.nil?
      raise ArgumentError, "base specified for non string value" unless String === object
      base = Integer.__index__(base)
      raise ArgumentError, "invalid radix #{base}" if base < 0 || base == 1 || base > 36
    end
    case object
    when Integer then object
    when String
      value = object.__strict_integer__(base || 0)
      raise ArgumentError, "invalid value for Integer(): #{object.inspect}" if value.nil?
      value
    when Float then __float_to_i__(object)
    when nil then raise TypeError, "can't convert nil into Integer"
    else
      if object.respond_to?(:to_int, true)
        converted = object.__send__(:to_int)
        return converted if Integer === converted
      end
      if object.respond_to?(:to_i, true)
        converted = object.__send__(:to_i)
        return converted if Integer === converted
        raise TypeError,
              "can't convert #{object.class} to Integer (#{object.class}#to_i gives #{converted.class})"
      end
      raise TypeError, "can't convert #{object.class} into Integer"
    end
  end

  def String(object)
    return object if String === object
    [:to_str, :to_s].each do |name|
      next unless object.respond_to?(name, true)
      converted = object.__send__(name)
      return converted if String === converted
      next if name == :to_str && converted.nil?
      raise TypeError,
            "can't convert #{object.class} to String (#{object.class}##{name} gives #{converted.class})"
    end
    raise TypeError, "can't convert #{object.class} into String"
  end

  # Whole seconds slept, rounded. A Rational — anything with `divmod` — is
  # read through `divmod(1)`; no duration at all would wait for a wakeup from
  # another thread, which needs threads (#45).
  def sleep(*args)
    if args.size > 1
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)"
    end
    duration = args[0]
    __needs_threads__ if duration.nil?
    seconds =
      if Integer === duration || Float === duration
        duration.to_f
      elsif !(String === duration) && duration.respond_to?(:divmod)
        whole, fraction = duration.divmod(1)
        whole.to_f + fraction.to_f
      else
        raise TypeError, "can't convert #{duration.class} into time interval"
      end
    raise ArgumentError, "time interval must not be negative" if seconds < 0
    __sleep__(seconds)
  end

  # `Kernel`'s functions: private instance methods, public on `Kernel` itself.
  module_function :Array, :Hash, :Integer, :String, :sleep, :__method__, :__callee__, :__dir__
end

# `Proc#to_s`: the address, where the block was written, and `(lambda)` for
# a lambda — BINARY, as `Kernel#to_s` is. Measured.
class Proc
  def to_s
    text = "#<Proc:0x" + __address__
    location = __proc_location__(self)
    text = text + " " + location[0] + ":" + location[1].to_s unless location.nil?
    text = text + " (lambda)" if lambda?
    (text + ">").__force_encoding__(0)
  end

  def inspect
    to_s
  end
end

# `exit` and `abort` only raise `SystemExit`, which is the whole of what
# happens until something rescues it. The rest of process control is
# `Process` (#43); the methods exist so a class can name them.
module Kernel
  def exit(status = true)
    raise SystemExit.new(Kernel.__exit_status__(status), "exit")
  end

  def abort(message = nil)
    return raise(SystemExit.new(1, "exit")) if message.nil?
    raise TypeError, "no implicit conversion of #{message.class} into String" unless String === message || message.respond_to?(:to_str)
    message = message.to_str unless String === message
    $stderr.puts(message)
    raise SystemExit.new(1, message)
  end

  def exit!(status = false)
    __needs_process__
  end

  def fork
    __needs_process__
  end

  def system(*)
    __needs_process__
  end

  module_function :exit, :abort, :exit!, :fork, :system

  def self.__exit_status__(status)
    return 0 if status == true
    return 1 if status == false
    return status if Integer === status
    raise TypeError, "no implicit conversion of #{status.nil? ? "nil" : status.class} into Integer" unless status.respond_to?(:to_int)
    status.to_int
  end
end
