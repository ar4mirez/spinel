# Ractors (#117). Each runs on a heap and an OS thread of its own; what goes
# between two goes as a copy, made by the sender and built again by the
# receiver (`crates/spinel-vm/src/ractor.rs`). The objects here hold ids into
# the program's registry of Ractors and ports, so two `Ractor` objects for one
# Ractor are `==`.
#
# Measured against CRuby 4.0.9: the Ruby 4 API, with ports in place of
# `Ractor.yield` and `#take`.
#
# ponytail: a shareable object a Ractor is sent, or reads from a constant,
# is a copy rather than the sender's object, so `equal?` and `object_id`
# answer otherwise than CRuby's (decision 2a). `Ractor.make_shareable` and
# the shareable procs are #118's.
class Ractor
  class Error < RuntimeError; end

  class RemoteError < Error
    attr_reader :ractor

    def initialize(message = nil, ractor: nil)
      super(message)
      @ractor = ractor
    end
  end

  class ClosedError < StopIteration; end
  class IsolationError < Error; end
  class MovedError < Error; end
  class UnsafeError < Error; end

  # What a sender is left holding after `send(object, move: true)`: every
  # call raises.
  class MovedObject < BasicObject
    def method_missing(*) = ::Kernel.raise(::Ractor::MovedError, "can not send any methods to a moved object")

    %i[! != == __id__ __send__ equal? instance_eval instance_exec].each do |name|
      define_method(name) { |*| ::Kernel.raise(::Ractor::MovedError, "can not send any methods to a moved object") }
    end
  end

  # A queue of messages that only the Ractor that made it receives from.
  class Port
    def self.__for__(id)
      port = allocate
      port.__send__(:__id__=, id)
      port.freeze
    end

    def initialize
      @__id__ = __ractor_port_new__
      freeze
    end

    def __port_id__ = @__id__

    def send(object, move: false)
      unless __ractor_port_send__(@__id__, object, move)
        raise Ractor::ClosedError, "The port was already closed"
      end
      self
    end
    alias << send

    def receive
      unless __ractor_port_owner__(@__id__) == __ractor_id__
        raise Ractor::Error, "only allowed from the creator Ractor of this port"
      end
      while true
        generation = __ractor_generation__
        taken = __ractor_port_take__(@__id__)
        return Ractor.__built__(taken) if Array === taken
        raise Ractor::ClosedError, "The port was already closed" if taken == :closed
        __ractor_wait__(generation)
      end
    end

    def close
      unless __ractor_port_owner__(@__id__) == __ractor_id__
        raise Ractor::Error, "closing port by other ractors is not allowed"
      end
      __ractor_port_close__(@__id__)
    end

    def closed? = __ractor_port_closed__(@__id__)

    def ==(other) = Port === other && other.__port_id__ == @__id__
    alias eql? ==
    def hash = [Port, @__id__].hash

    def inspect = "#<Ractor::Port to:##{__ractor_port_owner__(@__id__)} id:#{@__id__}>"

    private

    def __id__=(id)
      @__id__ = id
    end
  end

  def self.__for__(id)
    ractor = allocate
    ractor.__send__(:__ractor_id__=, id)
    ractor.default_port
    ractor.freeze
  end

  # A copied message, as `__ractor_*__` built it: `[value, hashes, frozen]`.
  # A Hash keyed by identity is rehashed here, and what was frozen frozen.
  def self.__built__(built)
    value, hashes, frozen = built
    hashes.each(&:rehash)
    __ractor_freeze__(frozen)
    value
  end

  def self.new(*args, name: nil, &block)
    raise ArgumentError, "must be called with a block" if block.nil?
    unless name.nil?
      raise TypeError, "no implicit conversion of #{name.class} into String" unless String === name
    end
    if __ractor_warn_once__ && Warning[:experimental]
      warn("Ractor API is experimental and may change in future versions of Ruby.", uplevel: 1, category: :experimental)
    end
    location = caller_locations(1, 1).first
    location = location && "#{location.path}:#{location.lineno}"
    __for__(__ractor_new__(args, name, block, location))
  end

  def self.current = (@__current__ ||= __for__(__ractor_id__))
  def self.main = (@__main__ ||= __for__(1))
  def self.main? = __ractor_id__ == 1
  def self.count = __ractor_count__

  def self.receive = current.default_port.receive
  def self.recv = receive

  def self.shareable?(object) = __ractor_shareable__(object)

  # `object` frozen, and everything it holds, as the standard library asks
  # before it keeps a value in a constant.
  #
  # ponytail: it freezes what it reaches and does not check a `Proc` for the
  # variables it captures, as CRuby's does; that check, and the shareable
  # procs, are #118's.
  def self.make_shareable(object, copy: false)
    object = __make_shareable_copy__(object) if copy
    __ractor_freeze_deep__(object).each(&:freeze)
    object
  end

  def self.shareable_proc(self: nil, &block) = block.freeze
  def self.shareable_lambda(self: nil, &block) = lambda(&block).freeze

  # The first of `ports` with a message, as `[port, message]`; a Ractor
  # among them is waited for to end, as `[ractor, value]`.
  def self.select(*ports)
    raise ArgumentError, "specify at least one ractor or `yield_value`" if ports.empty?
    while true
      generation = __ractor_generation__
      ports.each do |port|
        if Ractor === port
          outcome = __ractor_outcome__(port.__ractor_id__)
          return [port, port.__send__(:__outcome__, outcome)] unless outcome.nil?
        else
          taken = __ractor_port_take__(port.__port_id__)
          return [port, __built__(taken)] if Array === taken
          raise Ractor::ClosedError, "The port was already closed" if taken == :closed
        end
      end
      __ractor_wait__(generation)
    end
  end

  # Ractor-local storage, the running Ractor's own.
  def self.[](key) = (@__locals__ ||= {})[key.to_sym]
  def self.[]=(key, value)
    (@__locals__ ||= {})[key.to_sym] = value
  end

  def self.store_if_absent(key)
    locals = (@__locals__ ||= {})
    key = key.to_sym
    return locals[key] if locals.key?(key)
    locals[key] = yield(key)
  end

  # The bottom of every Ractor but the main one: what its heap runs first
  # (`ractor.rs`). Its standard streams are its own, for the same
  # descriptors, and the block runs with the Ractor as `self`.
  def self.__child_main__
    $stdin = IO.allocate.__setup__(0, "r", { path: "<STDIN>", autoclose: false }).__stdio__
    $stdout = IO.allocate.__setup__(1, "w", { path: "<STDOUT>", autoclose: false }).__stdio__
    $stderr = IO.allocate.__setup__(2, "w", { path: "<STDERR>", autoclose: false }).__stdio__
    $stderr.sync = true
    $VERBOSE = false
    $/ = IO.__default_rs__
    block, args = __ractor_start__(current)
    args = __built__(args)
    begin
      value = block.call(*args)
      $stdout.flush
      __ractor_finish__(true, value)
    rescue Exception => e
      begin
        $stdout.flush
      rescue Exception
        nil
      end
      if Thread.current.report_on_exception
        Thread.current.__send__(:__report__, e)
      end
      __ractor_finish__(false, e)
    end
  ensure
    Thread.__terminate_all__
  end

  def __ractor_id__ = @__id__

  def ==(other) = Ractor === other && other.__ractor_id__ == @__id__
  alias eql? ==
  def hash = [Ractor, @__id__].hash

  def name = __ractor_info__(@__id__)[0]

  def default_port
    return @__port__ if frozen?
    @__port__ = Port.__for__(__ractor_port__(@__id__))
  end

  def send(object, move: false)
    default_port.send(object, move: move)
    self
  end
  alias << send

  def close
    default_port.close
  end

  def value
    while true
      generation = __ractor_generation__
      outcome = __ractor_outcome__(@__id__)
      return __outcome__(outcome) unless outcome.nil?
      __ractor_wait__(generation)
    end
  end

  def join
    value
    self
  end

  def monitor(port)
    __ractor_monitor__(@__id__, port.__port_id__, true)
    self
  end

  def unmonitor(port)
    __ractor_monitor__(@__id__, port.__port_id__, false)
    self
  end

  def [](key) = Ractor.current == self ? Ractor[key] : raise(Ractor::Error, "only allowed from the Ractor itself")
  def []=(key, value)
    raise Ractor::Error, "only allowed from the Ractor itself" unless Ractor.current == self
    Ractor[key] = value
  end

  def inspect
    name, location, status = __ractor_info__(@__id__)
    out = "#<Ractor:##{@__id__}"
    out << " #{name}" unless name.nil?
    out << " #{location}" unless location.nil?
    out << " #{status}>"
  end
  alias to_s inspect

  private

  def __ractor_id__=(id)
    @__id__ = id
  end

  def __outcome__(outcome)
    ok, built = outcome
    value = Ractor.__built__(built)
    return value if ok
    raise Ractor::RemoteError.new("thrown by remote Ractor.", ractor: self), cause: value
  end
end

__ractor_register__(Ractor::MovedObject, [
  [Proc, "TypeError", "allocator undefined for Proc"],
  [Method, "TypeError", "allocator undefined for Method"],
  [UnboundMethod, "TypeError", "allocator undefined for UnboundMethod"],
  [Binding, "TypeError", "allocator undefined for Binding"],
  [Thread, "TypeError", "allocator undefined for Thread"],
  [Fiber, "TypeError", "allocator undefined for Fiber"],
  [Enumerator, "Ractor::Error", "can not copy Enumerator object."],
])
