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
  # class whose `name` is nil, and `"#<" + nil` is a TypeError.
  #
  # ponytail: Ruby puts the address in here too — `#<Foo:0x...>`. #15 left that
  # out rather than invent one, and this slice does not change it.
  def to_s
    "#<" + self.class.to_s + ">"
  end

  def inspect
    to_s
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
    i = 0
    while i < lines.size
      line = lines[i]
      text = line.nil? ? "" : line.to_s
      __write__(text)
      __write__("\n") unless text.end_with?("\n")
      i = i + 1
    end
    nil
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

  # The module functions (#161). Each becomes a private instance method and a
  # public singleton method, which is why `defined?(Object.print)` is nil while
  # `defined?(Kernel.puts)` is "method" — both measured on ruby 4.0.6.
  # `Kernel.private_instance_methods(false)` is where the list comes from; the
  # primitive half of it — `raise`, `proc`, `lambda`, `catch`, `throw`,
  # `block_given?` — is marked in `interp.rs`, beside where those are defined.
  module_function :loop, :p, :print, :puts
end
