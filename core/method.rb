# Method and UnboundMethod (#27) — a method taken out of its class and kept.
#
# Both are Ruby over two primitives. `__reflect_method_lookup__` answers what
# a name resolves to: the module that owns it, its arity, its visibility and
# where it was written. `__method_call__` runs the method that owner itself
# defines, on a receiver, whatever the receiver's class would now find under
# the name — which is what makes `super_method` and a rebound
# `UnboundMethod` call the body they say they hold.
#
# ponytail: a method redefined on its owner after it was captured runs the
# new body, because what is kept is the owner and the name, not the body.
# CRuby keeps the body. `original_name` is the name it was found under, so an
# alias reports itself. Both go when a Method holds its definition.

# What a lookup found: owner, arity, visibility, path, line, parameters, the
# definition's own number, and the name it was first defined under.
class Method
  def self.__make__(receiver, name, found)
    method = allocate
    method.__setup__(receiver, name, found)
    method
  end

  # Hidden names, so `instance_variables` on a Method is the program's own.
  def __setup__(receiver, name, found)
    @__receiver__ = receiver
    @__name__ = name
    @__found__ = found
    # Read by `define_method`, which takes the body this names.
    @__owner__ = found[0]
  end

  def __found__ = @__found__

  def receiver = @__receiver__
  def name = @__name__
  def owner = @__found__[0]
  def arity = @__found__[1]

  def original_name
    @__found__[7] || @__name__
  end

  def parameters
    Method.__parameters__(@__found__[5], @__found__[1])
  end

  # A primitive has no names to give: Ruby reports `[[:rest]]` for one that
  # takes any number, and a `[:req]` for each it requires.
  def self.__parameters__(parameters, arity)
    return parameters.map { |pair| pair.dup } unless parameters.nil?
    return [[:rest]] if arity < 0
    Array.new(arity) { [:req] }
  end

  # `a, b=..., *c, d:, e: ..., **f, &g`, as `inspect` shows a signature.
  def self.__signature__(parameters, arity)
    Method.__parameters__(parameters, arity).map do |kind, name|
      case kind
      when :req then (name || "_").to_s
      when :opt then "#{name || "_"}=..."
      when :rest then "*#{name == :* ? "" : name}"
      when :keyreq then "#{name}:"
      when :key then "#{name}: ..."
      when :keyrest then "**#{name == :** ? "" : name}"
      when :nokey then "**nil"
      when :block then "&#{name == :& ? "" : name}"
      end
    end.join(", ")
  end

  def self.__inspect__(kind, receiver_text, name, found)
    where = found[3].nil? ? "" : " #{found[3]}:#{found[4]}"
    original = found[7]
    shown = original.nil? || original == name ? name.to_s : "#{name}(#{original})"
    "#<#{kind}: #{receiver_text}#{shown}(#{Method.__signature__(found[5], found[1])})#{where}>"
  end

  def call(*args, **keywords, &block)
    __method_call__(@__found__[0], @__name__, @__receiver__, *args, **keywords, &block)
  end
  alias [] call
  alias === call

  def source_location
    @__found__[3].nil? ? nil : [@__found__[3], @__found__[4]]
  end

  def unbind
    UnboundMethod.__make__(@__name__, @__found__)
  end

  def super_method
    found = __reflect_method_lookup__(@__receiver__, @__found__[0], @__name__, true)
    found.nil? ? nil : Method.__make__(@__receiver__, @__name__, found)
  end

  # A lambda, as CRuby's is, that calls through to this.
  def to_proc
    method = self
    lambda { |*args, **keywords, &block| method.call(*args, **keywords, &block) }
  end

  def curry(*arity)
    to_proc.curry(*arity)
  end

  def >>(other)
    to_proc >> other
  end

  def <<(other)
    to_proc << other
  end

  # The same body on the same receiver: an alias is equal to what it aliases.
  def ==(other)
    other.instance_of?(Method) && @__receiver__.equal?(other.receiver) &&
      @__found__[6] == other.__found__[6]
  end
  alias eql? ==

  def hash
    @__found__[6].hash ^ @__receiver__.__id__
  end

  def inspect
    # A method on the receiver's own singleton class is written `recv.name`.
    owner = @__found__[0]
    text = __reflect_is_singleton__(owner) ? "#{@__receiver__.inspect}." : "#{owner}#"
    Method.__inspect__("Method", text, @__name__, @__found__)
  end
  alias to_s inspect
end

class UnboundMethod
  def self.__make__(name, found)
    method = allocate
    method.__setup__(name, found)
    method
  end

  def __setup__(name, found)
    @__name__ = name
    @__found__ = found
    @__owner__ = found[0]
  end

  def __found__ = @__found__

  def name = @__name__
  def owner = @__found__[0]
  def arity = @__found__[1]

  def original_name
    @__found__[7] || @__name__
  end

  def parameters
    Method.__parameters__(@__found__[5], @__found__[1])
  end

  def source_location
    @__found__[3].nil? ? nil : [@__found__[3], @__found__[4]]
  end

  # A module's methods bind to anything; a class's only to its own kind.
  def bind(receiver)
    owner = @__found__[0]
    if owner.instance_of?(Class) && !receiver.kind_of?(owner)
      raise TypeError, "bind argument must be an instance of #{owner}"
    end
    Method.__make__(receiver, @__name__, @__found__)
  end

  def bind_call(receiver, *args, **keywords, &block)
    bind(receiver).call(*args, **keywords, &block)
  end

  def ==(other)
    other.instance_of?(UnboundMethod) && @__found__[6] == other.__found__[6]
  end
  alias eql? ==

  def hash
    @__found__[6].hash
  end

  def inspect
    Method.__inspect__("UnboundMethod", "#{owner}#", @__name__, @__found__)
  end
  alias to_s inspect
end

module Kernel
  def method(name)
    name = Kernel.__method_name__(name)
    found = __reflect_method_lookup__(self, nil, name, false)
    if found.nil?
      raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
    end
    Method.__make__(self, name, found)
  end

  def public_method(name)
    name = Kernel.__method_name__(name)
    found = __reflect_method_lookup__(self, nil, name, false)
    if found.nil?
      raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
    end
    unless found[2] == :public
      raise NameError.new("method '#{name}' for #{Kernel.__method_home__(self)} is #{found[2]}", name, receiver: self)
    end
    Method.__make__(self, name, found)
  end

  def singleton_method(name)
    name = Kernel.__method_name__(name)
    klass = __reflect_class_of__(self)
    found = __reflect_is_singleton__(klass) ? __reflect_method_lookup__(self, nil, name, false) : nil
    if found.nil? || !found[0].equal?(klass)
      raise NameError.new("undefined singleton method '#{name}' for '#{inspect}'", name, receiver: self)
    end
    Method.__make__(self, name, found)
  end

  def self.__method_name__(name)
    return name if Symbol === name
    return name.to_sym if String === name
    return name.to_str.to_sym if name.respond_to?(:to_str)
    raise TypeError, "#{name.inspect} is not a symbol nor a string"
  end

  # How CRuby names where it looked: `class 'A'`, `module 'M'`, and for an
  # object the class it is an instance of.
  def self.__method_home__(object)
    home = Module === object ? object : object.class
    "#{Class === home ? "class" : "module"} '#{home}'"
  end
end

class Module
  def instance_method(name)
    name = Kernel.__method_name__(name)
    found = __reflect_method_lookup__(nil, self, name, false)
    if found.nil?
      raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
    end
    UnboundMethod.__make__(name, found)
  end

  def public_instance_method(name)
    name = Kernel.__method_name__(name)
    found = __reflect_method_lookup__(nil, self, name, false)
    if found.nil?
      raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
    end
    unless found[2] == :public
      raise NameError.new("method '#{name}' for #{Kernel.__method_home__(self)} is #{found[2]}", name, receiver: self)
    end
    UnboundMethod.__make__(name, found)
  end
end

class Symbol
  # `map(&:name)`. A lambda that sends the symbol to its first argument, as a
  # public call: `:puts.to_proc.call(1)` is a NoMethodError.
  def to_proc
    name = self
    lambda do |*args, **keywords, &block|
      raise ArgumentError, "no receiver given" if args.empty?
      receiver = args[0]
      rest = args.__take__(1, args.size - 1)
      receiver.public_send(name, *rest, **keywords, &block)
    end
  end
end

class Proc
  # `f >> g` calls `f` and hands what it answers to `g`; `f << g` the other
  # way round. The result is a lambda when the receiver is.
  def >>(other)
    raise TypeError, "callable object is expected" unless other.respond_to?(:call)
    first = self
    if lambda?
      lambda { |*args, **keywords, &block| other.call(first.call(*args, **keywords, &block)) }
    else
      proc { |*args, **keywords, &block| other.call(first.call(*args, **keywords, &block)) }
    end
  end

  def <<(other)
    raise TypeError, "callable object is expected" unless other.respond_to?(:call)
    last = self
    # The lambda-ness of whichever runs first, which here is `other`: anything
    # callable that is not a plain proc counts as a lambda. Measured.
    if !other.instance_of?(Proc) || other.lambda?
      lambda { |*args, **keywords, &block| last.call(other.call(*args, **keywords, &block)) }
    else
      proc { |*args, **keywords, &block| last.call(other.call(*args, **keywords, &block)) }
    end
  end

  # A lambda that collects arguments until it has `arity` of them and then
  # calls. A lambda's own arity bounds what may be asked for; a proc's does not.
  def curry(*given)
    if given.size > 1
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..1)"
    end
    required = arity < 0 ? -arity - 1 : arity
    wanted = given.empty? ? required : Integer.__index__(given[0])
    if lambda? && !given.empty?
      if arity >= 0 ? wanted != arity : wanted < required
        expected = arity >= 0 ? arity.to_s : "#{required}+"
        raise ArgumentError, "wrong number of arguments (given #{wanted}, expected #{expected})"
      end
    end
    Proc.__curry__(self, wanted, [])
  end

  # Curried, it is what it was: a lambda stays one and a proc stays a proc.
  def self.__curry__(callable, wanted, held)
    if callable.lambda?
      lambda do |*more, &block|
        all = held + more
        all.size >= wanted ? callable.call(*all, &block) : Proc.__curry__(callable, wanted, all)
      end
    else
      proc do |*more, &block|
        all = held + more
        all.size >= wanted ? callable.call(*all, &block) : Proc.__curry__(callable, wanted, all)
      end
    end
  end
end
