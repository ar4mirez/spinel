# Method and UnboundMethod (#27) — a method taken out of its class and kept.
#
# Both are Ruby over two primitives. `__reflect_method_lookup__` answers what
# a name resolves to: the module that owns it, its arity, its visibility and
# where it was written. `__method_call__` runs the definition the lookup
# found, on a receiver, whatever the receiver's class — or the owner itself —
# would now find under the name: which is what makes `super_method` and a
# rebound `UnboundMethod` call the body they say they hold.

# What a lookup found: owner, arity, visibility, path, line, parameters, the
# definition's own number, the name it was first defined under, and the
# lexical scope its constants resolve in. The definition is what `call` runs,
# so a method redefined after it was captured still runs the body it was.
class Method
  # `klass` is the class `inspect` shows it was looked up from, when that is
  # not the receiver's: `super_method` finds it in the owner.
  def self.__make__(receiver, name, found, klass = nil)
    method = allocate
    method.__setup__(receiver, name, found, klass)
    method
  end

  # Hidden names, so `instance_variables` on a Method is the program's own.
  def __setup__(receiver, name, found, klass = nil)
    @__receiver__ = receiver
    @__name__ = name
    @__klass__ = klass
    # Also read by `define_method`, which takes the definition it holds.
    @__found__ = found
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
    # A name the receiver only answers through `method_missing`.
    if @__found__[6].is_a?(Symbol)
      return @__receiver__.__send__(:method_missing, @__name__, *args, **keywords, &block)
    end
    found = @__found__
    __method_call__(found[0], @__name__, @__receiver__, found[6], found[8], *args, **keywords, &block)
  end
  alias [] call
  alias === call

  def source_location
    @__found__[3].nil? ? nil : [@__found__[3], @__found__[4]]
  end

  def unbind
    UnboundMethod.__make__(@__name__, @__found__, __reflect_class_of__(@__receiver__))
  end

  def super_method
    name = original_name
    found = __reflect_method_lookup__(@__receiver__, @__found__[0], name, true)
    found.nil? ? nil : Method.__make__(@__receiver__, name, found, found[0])
  end

  # A lambda, as CRuby's is, that calls through to this and reports this
  # method's arity, parameters and location as its own.
  def to_proc
    method = self
    made = lambda { |*args, **keywords, &block| method.call(*args, **keywords, &block) }
    Proc.__stands_for__(made, arity, parameters, source_location, nil, self)
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
    # Otherwise the class it was looked up from, and the owner beside it when
    # that is another module: a singleton class only when it is a class's or
    # a module's, since an object's is not where its methods were found.
    owner = @__found__[0]
    if __reflect_is_singleton__(owner)
      text = "#{@__receiver__.inspect}."
    else
      klass = @__klass__
      if klass.nil?
        klass = __reflect_class_of__(@__receiver__)
        klass = Kernel.__class_of__(@__receiver__) unless Module === @__receiver__
      end
      text = klass.equal?(owner) ? "#{owner}#" : "#{klass}(#{owner})#"
    end
    Method.__inspect__("Method", text, @__name__, @__found__)
  end
  alias to_s inspect
end

class UnboundMethod
  # `from` is the module the lookup started at, whose ancestry
  # `super_method` walks: the owner's own may not hold what `super` reaches.
  def self.__make__(name, found, from = found[0])
    method = allocate
    method.__setup__(name, found, from)
    method
  end

  def __setup__(name, found, from)
    @__name__ = name
    @__found__ = found
    @__from__ = from
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

  def super_method
    name = original_name
    found = __reflect_method_lookup__(nil, @__found__[0], name, true, @__from__)
    found.nil? ? nil : UnboundMethod.__make__(name, found, @__from__)
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
      # A name `respond_to_missing?` claims is a method all the same: one
      # that calls `method_missing`, takes anything, and is equal to another
      # of the same name. Its place in the lookup is the name itself.
      if respond_to_missing?(name, true)
        return Method.__make__(self, name, [self.class, -1, :public, nil, nil, nil, name, name])
      end
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
    UnboundMethod.__make__(name, found, self)
  end

  # Marks each named method so that the keywords its rest parameter collects
  # arrive as a Hash flagged to be passed on as keywords again (#200). The flag
  # is on the body: an alias shares it whichever side of the mark it was made
  # on. A module's lookup falls back to `Object`'s, and only the module that
  # defines the method may mark it. CRuby's checks and warnings, measured.
  def ruby2_keywords(*names)
    names.each do |name|
      name = Kernel.__method_name__(name)
      found = __reflect_method_lookup__(nil, self, name, false)
      found = __reflect_method_lookup__(nil, Object, name, false) if found.nil? && !(Class === self)
      if found.nil?
        raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
      end
      reason =
        if !found[0].equal?(self)
          "can only set in method defining module"
        elsif found[5].nil?
          "method not defined in Ruby"
        else
          Proc.__ruby2_keywords_refusal__(found[5], "method")
        end
      if reason.nil?
        __reflect_ruby2_keywords__(found[6])
      else
        __warning__("Skipping set of ruby2_keywords flag for #{name} (#{reason})")
      end
    end
    nil
  end
  private :ruby2_keywords

  def public_instance_method(name)
    name = Kernel.__method_name__(name)
    found = __reflect_method_lookup__(nil, self, name, false)
    if found.nil?
      raise NameError.new("undefined method '#{name}' for #{Kernel.__method_home__(self)}", name, receiver: self)
    end
    unless found[2] == :public
      raise NameError.new("method '#{name}' for #{Kernel.__method_home__(self)} is #{found[2]}", name, receiver: self)
    end
    UnboundMethod.__make__(name, found, self)
  end
end

class Symbol
  # `map(&:name)`. A lambda that sends the symbol to its first argument, as a
  # public call: `:puts.to_proc.call(1)` is a NoMethodError.
  def to_proc
    name = self
    made = lambda do |*args, **keywords, &block|
      raise ArgumentError, "no receiver given" if args.empty?
      receiver = args[0]
      rest = args.__take__(1, args.size - 1)
      receiver.public_send(name, *rest, **keywords, &block)
    end
    Proc.__stands_for__(made, -2, [[:req], [:rest]], nil, "(&#{inspect})")
  end
end

class Proc
  # `Proc.new { }` is the block it was given, and on a subclass a copy of it
  # wearing the subclass — unless it already is one. `initialize` runs either
  # way, on what is answered, and is not what makes it a Proc.
  def self.new(*args, &block)
    raise ArgumentError, "tried to create Proc object without a block" if block.nil?
    made = block.instance_of?(self) ? block : __proc_with_class__(block, self)
    made.__send__(:initialize, *args, &block)
    made
  end

  # A proc the core library builds around another callable — `curry`, `>>`,
  # `Method#to_proc`, `Symbol#to_proc` — is a C function's proc in CRuby. It
  # reports what it stands for rather than the Ruby block it is here: an
  # arity, a parameter list, a location, a label for `inspect`, and the
  # Method it calls, if it calls one, which is the only one with a binding.
  def self.__stands_for__(made, arity, parameters, location, label = nil, method = nil)
    made.__ivar_set__(:@__callable__, [arity, parameters, location, label, method])
    made
  end

  alias __arity__ arity
  private :__arity__

  def arity
    callable = @__callable__
    callable.nil? ? __arity__ : callable[0]
  end

  # A proc's required parameters are optional ones, since it fills in what it
  # is not given; `lambda:` says which rule to report by. A destructured
  # parameter has no name of its own.
  def parameters(lambda: nil)
    callable = @__callable__
    return callable[1].map { |pair| pair.dup } unless callable.nil?
    list = __proc_parameters__(self)
    strict = lambda.nil? ? lambda? : lambda
    return list if strict
    list.map { |pair| pair[0] == :req ? [:opt, *pair.__take__(1, pair.size - 1)] : pair }
  end

  def source_location
    callable = @__callable__
    location = callable.nil? ? __proc_location__(self) : callable[2]
    location.nil? ? nil : location.dup
  end

  def binding
    callable = @__callable__
    return __proc_binding__(self) if callable.nil?
    method = callable[4]
    raise ArgumentError, "Can't create Binding from C level Proc" if method.nil?
    file, line = callable[2] || ["(binding)", 1]
    binding = Proc.__empty_binding__.__binding_receiver_set__(method.receiver)
    binding.__binding_place_set__(method.__found__[8], file, line)
  end

  # A binding with no locals, for a method's proc to lend its scope to.
  def self.__empty_binding__ = __proc_binding__(proc {})

  def to_proc = self

  # Only a Ruby block with a rest parameter, no post parameters and no
  # keywords can take the flag; anything else is warned about and left alone,
  # with the reason CRuby gives. Measured. The flag is on the block's body, so
  # a `dup` made before or after shares it, as every proc of one literal does
  # (#200).
  def ruby2_keywords
    raise FrozenError.new("can't modify frozen Proc: " + inspect, receiver: self) if frozen?
    callable = @__callable__
    unless callable.nil?
      reason = callable[4].nil? ? "proc not defined in Ruby" : "proc created from method"
      __warning__("Skipping set of ruby2_keywords flag for proc (#{reason})")
      return self
    end
    reason = Proc.__ruby2_keywords_refusal__(parameters, "proc")
    if reason.nil?
      __reflect_ruby2_keywords__(self)
    else
      __warning__("Skipping set of ruby2_keywords flag for proc (#{reason})")
    end
    self
  end

  # Why a body with these `parameters` cannot take the `ruby2_keywords` flag,
  # or nil when it can: it needs a rest parameter, and no post parameters or
  # keywords. A post parameter is whatever follows the rest, which a proc
  # reports as `:opt`. CRuby's wording, `what` being "method" or "proc".
  def self.__ruby2_keywords_refusal__(parameters, what)
    kinds = parameters.map { |pair| pair[0] }
    rest = kinds.index(:rest)
    keywords = !(kinds & [:key, :keyreq, :keyrest]).empty?
    post = !rest.nil? && !(kinds.__take__(rest + 1, kinds.size - rest - 1) & [:req, :opt]).empty?
    return nil unless rest.nil? || keywords || post
    "#{what} accepts keywords or post arguments or #{what} does not accept argument splat"
  end

  # Equal procs are one body in one environment, so where it was written
  # and which rule it calls by agree whenever `==` does.
  def hash
    [source_location, lambda?].hash
  end

  # The address, where the block was written, and `(lambda)` for a lambda —
  # BINARY, as `Kernel#to_s` is. Measured.
  def to_s
    text = "#<Proc:0x" + __address__
    callable = @__callable__
    if callable.nil?
      location = __proc_location__(self)
      text = text + " " + location[0] + ":" + location[1].to_s unless location.nil?
    else
      # What it stands for is not where it was written: no location.
      text = text + callable[3] unless callable[3].nil?
    end
    text = text + " (lambda)" if lambda?
    (text + ">").__force_encoding__(0)
  end

  # `f >> g` calls `f` and hands what it answers to `g`; `f << g` the other
  # way round. The result is a lambda when the receiver is.
  def >>(other)
    raise TypeError, "callable object is expected" unless other.respond_to?(:call)
    first = self
    made = if lambda?
      lambda { |*args, **keywords, &block| other.call(first.call(*args, **keywords, &block)) }
    else
      proc { |*args, **keywords, &block| other.call(first.call(*args, **keywords, &block)) }
    end
    Proc.__stands_for__(made, -1, [[:rest]], nil)
  end

  def <<(other)
    raise TypeError, "callable object is expected" unless other.respond_to?(:call)
    last = self
    # The lambda-ness of whichever runs first, which here is `other`: anything
    # callable that is not a plain proc counts as a lambda. Measured.
    made = if !other.instance_of?(Proc) || other.lambda?
      lambda { |*args, **keywords, &block| last.call(other.call(*args, **keywords, &block)) }
    else
      proc { |*args, **keywords, &block| last.call(other.call(*args, **keywords, &block)) }
    end
    Proc.__stands_for__(made, -1, [[:rest]], nil)
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
      # Optional parameters raise the most it may be asked for, and keywords
      # one more, for the hash; a rest parameter takes the bound away.
      kinds = parameters.map { |pair| pair[0] }
      keywords = (kinds & [:key, :keyreq, :keyrest]).empty? ? 0 : 1
      most = kinds.include?(:rest) ? nil : kinds.count(:req) + kinds.count(:opt) + keywords
      if wanted < required || (!most.nil? && wanted > most)
        expected = if most.nil? then "#{required}+"
                   elsif most == required then required.to_s
                   else "#{required}..#{most}"
                   end
        raise ArgumentError, "wrong number of arguments (given #{wanted}, expected #{expected})"
      end
    end
    Proc.__curry__(self, wanted, [])
  end

  # Curried, it is what it was: a lambda stays one and a proc stays a proc.
  def self.__curry__(callable, wanted, held)
    made = if callable.lambda?
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
    Proc.__stands_for__(made, -1, [[:rest]], nil)
  end
end
