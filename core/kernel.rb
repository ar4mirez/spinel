# Kernel — the methods every object has.
#
# `send`, `raise`, `throw`, `catch`, `block_given?`, `proc`, `lambda`, `class`,
# `equal?`, `nil?`, `dup`, `freeze`, `frozen?`, `object_id` and `__write__` are
# primitives: each is dispatch, an allocation, a header bit, or a syscall.
# Everything below is Ruby, because Ruby can say it.
module Kernel
  def ===(other)
    self == other
  end

  # `class or module required` rather than `false`: asking whether an object is
  # a kind of `1` is a mistake in the caller, and Ruby says so. Measured.
  #
  # The check runs only when the answer would be false — a `mod` that is in the
  # ancestry is a module already — so the common path costs what it always did.
  # `mod.class.ancestors` rather than `mod.is_a?(Module)`, because this *is*
  # `is_a?` and asking it here would not terminate.
  def is_a?(mod)
    return true if self.class.ancestors.include?(mod)
    raise TypeError, "class or module required" unless mod.class.ancestors.include?(Module)
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
  def to_s
    "#<" + self.class.to_s + ":0x" + __address__ + ">"
  end

  # `to_s` plus the instance variables, measured on ruby 4.0.7:
  # `#<Foo:0x... @a=1, @b="x">`. It never calls `to_s`, which a subclass may
  # have overridden, and an object met again while inspecting itself prints as
  # `#<Foo:0x... ...>`. Ruby 4.0 lets a class choose which variables to show
  # with a private `instance_variables_to_inspect`.
  def inspect
    head = "#<" + self.class.to_s + ":0x" + __address__
    names = if respond_to?(:instance_variables_to_inspect, true)
      __send__(:instance_variables_to_inspect)
    else
      instance_variables.reject { |name| name.to_s.start_with?("@__") && name.to_s.end_with?("__") }
    end
    return head + ">" if names.empty?
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
    rescue StopIteration
      nil
    end
  end

  def puts(*lines)
    if lines.size == 0
      __write__("\n")
      return nil
    end
    __puts_lines__(lines, [])
    nil
  end

  # An Array argument is flattened into its elements, measured: `puts [1, [2]]`
  # is two lines, `puts []` is none, and an Array that contains itself prints
  # `[...]` where it recurs rather than looping.
  def __puts_lines__(lines, seen)
    i = 0
    while i < lines.size
      line = lines[i]
      if line.is_a?(Array)
        if seen.any? { |outer| outer.equal?(line) }
          __write__("[...]\n")
        else
          __puts_lines__(line, seen + [line])
        end
      else
        text = line.nil? ? "" : line.to_s
        __write__(text)
        __write__("\n") unless text.end_with?("\n")
      end
      i = i + 1
    end
  end

  def print(*parts)
    i = 0
    while i < parts.size
      __write__(parts[i].to_s)
      i = i + 1
    end
    nil
  end

  def p(*values)
    i = 0
    while i < values.size
      __write__(values[i].inspect)
      __write__("\n")
      i = i + 1
    end
    return nil if values.size == 0
    return values[0] if values.size == 1
    values
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
  module_function :loop, :p, :print, :puts
end
