# `TracePoint` (#127), for the five events the interpreter fires: `:line`,
# `:call`, `:return`, `:class` and `:raise`.
#
# This Ractor's enabled `TracePoint`s are `@__hooks__`, newest first, as
# CRuby connects a hook at the head of its list, and `__trace_events__` tells
# the interpreter the union of the events they wait for. While that is not
# zero the loop leaves its fast paths and, before every instruction, looks
# for an event there (`interp.rs`, "TracePoint"); each it finds is a call to
# `TracePoint.__fire__` pushed on top of the frame it happened in, which runs
# every hook that wants it. A hook runs with this thread's `__tracing__` set,
# so the events its own code makes fire nothing, as CRuby's do not, until
# `TracePoint.allow_reentry`.
#
# Asking for any other event is a refusal rather than a hook that never runs:
# a `TracePoint` that silently missed `:c_call` would be a wrong answer.
class Thread
  # Whether a `TracePoint` hook is running on this thread.
  def __tracing__ = @__tracing__

  def __tracing__=(value)
    @__tracing__ = value
  end
end

class Method
  # What `TracePoint#enable(target:)` hooks: the definition this `Method`
  # runs, or nil for one only `method_missing` answers.
  def __trace_body__
    body = @__found__[6]
    body.is_a?(Integer) ? body : nil
  end
end

class UnboundMethod
  def __trace_body__
    body = @__found__[6]
    body.is_a?(Integer) ? body : nil
  end
end

module Kernel
  # The older tracing API: a proc called for every event, `c-call` and
  # `c-return` among them, which nothing fires yet, so asking for one is a
  # refusal. Nil turns it off, which with none on is nothing to do.
  def set_trace_func(callback)
    return nil if callback.nil?
    raise TypeError, "trace_func needs to be Proc" unless callback.is_a?(Proc)
    Kernel.__needs_trace_events__
  end
  module_function :set_trace_func
end

class TracePoint
  # Every event CRuby names, so that one of the others is a refusal and a
  # misspelling is CRuby's `ArgumentError`.
  EVENTS = %i[
    line class end call return c_call c_return raise b_call b_return
    a_call a_return thread_begin thread_end fiber_switch script_compiled rescue
  ].freeze
  # The ones the interpreter fires, by the bit `__trace_events__` keeps each
  # in: `interp.rs`'s `TRACE_LINE` and the rest.
  BITS = { line: 1, call: 2, return: 4, class: 8, raise: 16 }.freeze
  private_constant :EVENTS, :BITS

  @__hooks__ = []

  class << self
    # As CRuby's, whose `TracePoint`s only `new` makes.
    def allocate
      raise TypeError, "allocator undefined for TracePoint"
    end

    # In CRuby's order: every name, then the block. No name at all is
    # CRuby's `RUBY_EVENT_TRACEPOINT_ALL`, which holds more than the five.
    def new(*events, &block)
      names = events.map do |name|
        event = TracePoint.__event_name__(name)
        raise ArgumentError, "unknown event: #{event}" unless EVENTS.include?(event)
        event
      end
      raise ArgumentError, "must be called with a block" if block.nil?
      Kernel.__needs_trace_events__ if names.empty?
      mask = 0
      names.each do |event|
        bit = BITS[event]
        Kernel.__needs_trace_events__ if bit.nil?
        mask |= bit
      end
      TracePoint.__made__(mask, block)
    end

    def trace(*events, &block)
      point = new(*events, &block)
      point.enable
      point
    end

    # Let the events a hook's own code makes fire, inside the block.
    def allow_reentry
      thread = Thread.current
      raise RuntimeError, "No need to allow reentrance." unless thread.__tracing__
      thread.__tracing__ = false
      begin
        yield
      ensure
        thread.__tracing__ = true
      end
    end
  end

  # `rb_to_symbol_type`: a Symbol, a String as one, or what `to_sym` answers
  # when that is a Symbol.
  def self.__event_name__(name)
    return name if name.is_a?(Symbol)
    return name.to_sym if name.is_a?(String)
    unless name.respond_to?(:to_sym, true)
      raise TypeError, "no implicit conversion of #{name.class} into Symbol"
    end
    symbol = name.__send__(:to_sym)
    return symbol if symbol.is_a?(Symbol)
    raise TypeError, "can't convert #{name.class} to Symbol (#{name.class}#to_sym gives #{symbol.class})"
  end

  def self.__made__(mask, block)
    point = __allocate__
    point.__init__(mask, block)
    point
  end

  def self.__allocate__ = Class.instance_method(:allocate).bind_call(self)

  def __init__(mask, block)
    @__events__ = mask
    @__block__ = block
    @__enabled__ = false
    # What `enable` was told and what the hook was connected with: CRuby
    # keeps the first even when an `enable` finds itself already enabled.
    @__target_thread__ = nil
    @__hook_thread__ = nil
    # `enable(target:)`: the bodies it hooks, and the line it keeps `:line`
    # to.
    @__targets__ = nil
    @__target_line__ = nil
    # The event its hook is running for, while it runs.
    @__event__ = nil
  end

  def enable(target: nil, target_line: nil, target_thread: :default)
    if target_thread == :default
      target_thread = block_given? && target.nil? && target_line.nil? ? Thread.current : nil
    end
    if target_thread.nil?
      @__target_thread__ = nil
    else
      unless target_thread.is_a?(Thread)
        raise TypeError, "wrong argument type #{target_thread.class} (expected VM/thread)"
      end
      raise ArgumentError, "can not override target_thread filter" unless @__target_thread__.nil?
      @__target_thread__ = target_thread
    end
    previous = @__enabled__
    if target.nil?
      raise ArgumentError, "only target_line is specified" unless target_line.nil?
      __connect__
    else
      __connect_for__(target, target_line)
    end
    return previous unless block_given?
    begin
      yield
    ensure
      previous ? __connect__ : __disconnect__
    end
  end

  def disable
    previous = @__enabled__
    return (__disconnect__; previous) unless block_given?
    raise ArgumentError, "can't disable a targeting TracePoint in a block" unless @__targets__.nil?
    __disconnect__
    begin
      yield
    ensure
      previous ? __connect__ : __disconnect__
    end
  end

  def enabled? = @__enabled__

  def event = __event__[0]
  def path = __event__[3]
  def lineno = __event__[4]
  def method_id = __event__[5]
  def callee_id = __event__[6]
  def defined_class = __event__[7]

  def self
    __event__[2]
  end

  def binding
    Kernel.__trace_binding__(__event__[1])
  end

  def parameters
    event = __event__
    raise RuntimeError, "not supported by this event" unless event[0] == :call || event[0] == :return
    Kernel.__trace_parameters__(event[1])
  end

  def return_value
    event = __event__
    raise RuntimeError, "not supported by this event" unless event[0] == :return
    event[8]
  end

  def raised_exception
    event = __event__
    raise RuntimeError, "not supported by this event" unless event[0] == :raise
    event[8]
  end

  # `:script_compiled`'s, which no `TracePoint` here waits for.
  def eval_script
    __event__
    raise RuntimeError, "not supported by this event"
  end

  def instruction_sequence
    __event__
    raise RuntimeError, "not supported by this event"
  end

  def inspect
    event = @__event__
    return @__enabled__ ? "#<TracePoint:enabled>" : "#<TracePoint:disabled>" if event.nil?
    name, _, _, path, line, method_id = event
    case name
    when :line
      method_id.nil? ? "#<TracePoint:line #{path}:#{line}>" : "#<TracePoint:line #{path}:#{line} in '#{method_id}'>"
    when :call, :return
      "#<TracePoint:#{name} '#{method_id}' #{path}:#{line}>"
    else
      "#<TracePoint:#{name} #{path}:#{line}>"
    end
  end

  def __event__
    event = @__event__
    raise RuntimeError, "access from outside" if event.nil?
    event
  end

  # `rb_tracepoint_enable`.
  def __connect__
    raise ArgumentError, "can't nest-enable a targeting TracePoint" unless @__targets__.nil?
    return if @__enabled__
    @__hook_thread__ = @__target_thread__
    @__enabled__ = true
    TracePoint.__hooked__(self, true)
  end

  # `rb_tracepoint_enable_for_target`: the target's body and every body
  # written inside it, which must have somewhere an event of this
  # `TracePoint`'s can fire.
  def __connect_for__(target, target_line)
    body = case target
           when Method, UnboundMethod then target.__trace_body__
           when Proc then target
           end
    found = body.nil? ? nil : Kernel.__trace_target__(body, @__events__, nil)
    raise ArgumentError, "specified target is not supported" if found.nil?
    raise ArgumentError, "can't nest-enable a targeting TracePoint" if @__enabled__
    unless target_line.nil?
      if @__events__ & BITS[:line] == 0
        raise ArgumentError, "target_line is specified, but line event is not specified"
      end
      target_line = Integer.__to_int__(target_line)
      found = Kernel.__trace_target__(body, @__events__, target_line)
    end
    ids, count = found
    raise ArgumentError, "can not enable any hooks" if count == 0
    @__targets__ = ids.to_h { |id| [id, true] }
    @__target_line__ = target_line
    @__hook_thread__ = nil
    @__enabled__ = true
    TracePoint.__hooked__(self, true)
  end

  # `rb_tracepoint_disable`.
  def __disconnect__
    TracePoint.__hooked__(self, false) if @__enabled__
    @__enabled__ = false
    @__targets__ = nil
    @__target_line__ = nil
    @__target_thread__ = nil
    @__hook_thread__ = nil
  end

  def self.__hooked__(point, enabled)
    hooks = @__hooks__.reject { |hook| hook.equal?(point) }
    hooks.unshift(point) if enabled
    @__hooks__ = hooks
    mask = 0
    hooks.each { |hook| mask |= hook.__events__ }
    Kernel.__trace_events__(mask)
  end

  def __events__ = @__events__

  # What the interpreter calls for each event, on top of the frame it
  # happened in: the event's name, that frame's id, `self`, where it is, the
  # method and class it is in, its return value or exception, and the
  # identity of the body it is in, which a targeting `TracePoint` asks about.
  def self.__fire__(event, frame, receiver, path, line, method_id, callee_id, defined_class, value, body)
    thread = Thread.current
    return if thread.__tracing__
    hooks = @__hooks__
    return if hooks.empty?
    bit = BITS[event]
    info = [event, frame, receiver, path, line, method_id, callee_id, defined_class, value].freeze
    hooks.each { |hook| hook.__run__(bit, info, thread, body) }
    nil
  end

  def __run__(bit, info, thread, body)
    return unless @__enabled__ && @__events__ & bit != 0
    return unless @__hook_thread__.nil? || @__hook_thread__.equal?(thread)
    unless @__targets__.nil?
      # A targeting `TracePoint`'s hooks are on its bodies' instructions,
      # which a `:raise` is not one of.
      return if bit == BITS[:raise] || !@__targets__.key?(body)
      return if bit == BITS[:line] && !@__target_line__.nil? && info[4] != @__target_line__
    end
    outer = @__event__
    tracing = thread.__tracing__
    @__event__ = info
    thread.__tracing__ = true
    begin
      @__block__.call(self)
    ensure
      thread.__tracing__ = tracing
      @__event__ = outer
    end
  end
end
