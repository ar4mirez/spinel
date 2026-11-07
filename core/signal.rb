# Signal (#29, #44).
#
# The table is the platform's and comes from `__signal_list__`: CRuby's
# `siglist` order, with this platform's numbers from the `libc` crate.
#
# A handler runs as CRuby's does: the OS handler only counts the signal
# (`spinel_vm::shared::signals`), and the main thread runs the Ruby handler
# at its next safe point — the interpreter's turn check, or the wait it is in,
# which goes on waiting afterwards. `__run_pending__` is what both call.
module Signal
  # A new Hash every call, as in CRuby, so a caller that mutates it cannot
  # change what the next caller sees.
  def self.list
    table = {}
    __signal_list__[0].each { |name, number| table[name] = number }
    table
  end

  # What `trap` answers is CRuby's, measured on 4.0.7 and 4.0.9, by signal
  # number so an alias shares its handler: the signals Ruby reserves start
  # "DEFAULT", which is Ruby's handler (a `SignalException` in the main
  # thread, an `Interrupt` for SIGINT); PIPE, ABRT and SYS hold a C handler
  # (nil); and the rest start at the operating system's "SYSTEM_DEFAULT".
  RESERVED = %w[HUP INT QUIT ALRM TERM CHLD USR1 USR2].freeze

  def self.trap(signal, command = nil, &block)
    name =
      case signal
      when Integer then signal == 0 ? "EXIT" : (signame(signal) || raise(ArgumentError, "invalid signal number (#{signal})"))
      else
        given =
          if Symbol === signal || String === signal
            signal.to_s
          elsif signal.respond_to?(:to_str)
            signal.to_str
          else
            raise ArgumentError, "bad signal type #{signal.class}"
          end
        raise ArgumentError, "negative signal name: #{given}" if given.start_with?("-")
        given.delete_prefix("SIG")
      end
    number = name == "EXIT" ? 0 : list[name]
    raise ArgumentError, "unsupported signal 'SIG#{name}'" if number.nil?
    if %w[KILL STOP SEGV BUS ILL FPE VTALRM].include?(name)
      raise ArgumentError, "can't trap reserved signal: SIG#{name}" unless %w[KILL STOP].include?(name)
      raise Errno::EINVAL, "SIG#{name}"
    end
    handler = block || command
    handler = handler.to_s if Symbol === handler
    handler =
      case handler
      when "DEFAULT", "SIG_DFL"
        RESERVED.include?(signame(number) || name) ? "DEFAULT" : "SYSTEM_DEFAULT"
      when "IGNORE", "SIG_IGN", "" then "IGNORE"
      else handler
      end
    __install__(number, handler) unless number == 0
    handlers = (@__handlers__ ||= {})
    previous = handlers.key?(number) ? handlers[number] : __initial_handler__(number)
    handlers[number] = handler
    previous
  end

  # Tell the operating system what `number` does now: count it for Ruby, or
  # leave it to the system's default, or ignore it. CHLD's "DEFAULT" is the
  # system's, since Ruby's own handler for it does nothing.
  def self.__install__(number, handler)
    mode =
      case handler
      when "SYSTEM_DEFAULT" then 0
      when "IGNORE", nil then 1
      when "DEFAULT" then signame(number) == "CHLD" ? 0 : 2
      else 2
      end
    result = __signal_handle__(number, mode)
    raise SystemCallError.new("sigaction", result) if Integer === result
  end

  # What would run for `number` now: a trapped handler, or the one it starts with.
  def self.__handler_for__(number)
    handlers = (@__handlers__ ||= {})
    handlers.key?(number) ? handlers[number] : __initial_handler__(number)
  end

  def self.__initial_handler__(number)
    name = signame(number)
    return nil if number == 0 || name == "ABRT" || name == "SYS" || name == "PIPE"
    RESERVED.include?(name) ? "DEFAULT" : "SYSTEM_DEFAULT"
  end

  # Every signal that arrived since the last look, handled in the order of
  # their numbers, once per arrival. Called by the main thread only, at a
  # safe point: the interpreter's turn check, or a wait a signal ended.
  def self.__run_pending__
    __signal_take__.each { |number| __deliver__(number) }
    nil
  end

  # Run what `number` is trapped to, as CRuby's `signal_exec` and
  # `rb_signal_exec` do: a command is called with the number, a String
  # command is evaluated, "EXIT" exits, and Ruby's default handler raises.
  def self.__deliver__(number)
    handler = __handler_for__(number)
    case handler
    when "DEFAULT"
      # SIGINT's is an `Interrupt` with no message, as `rb_interrupt` makes
      # it: it inspects and reports as "Interrupt". Measured on 4.0.9.
      Kernel.raise(signame(number) == "INT" ? Interrupt.new("") : SignalException.new(number))
    when "EXIT"
      Kernel.raise SystemExit.new(0, "exit")
    when "SYSTEM_DEFAULT", "IGNORE", nil
      # Changed after it arrived and before it was handled: what it is set
      # to now is the operating system's to act on, and it did not.
      nil
    when String
      TOPLEVEL_BINDING.eval(handler)
    else
      handler.call(number)
    end
  end

  # The `trap(:EXIT)` handler, run as the program ends and before the
  # `at_exit` blocks, as `rb_ec_teardown` runs it.
  def self.__run_exit__
    handler = (@__handlers__ ||= {})[0]
    @__handlers__.delete(0)
    case handler
    when nil, "DEFAULT", "SYSTEM_DEFAULT", "IGNORE", "EXIT" then nil
    when String then TOPLEVEL_BINDING.eval(handler)
    else handler.call(0)
    end
  end

  # The first name for `number` — "ABRT", not "IOT" — or nil.
  def self.signame(number)
    # CRuby's `NUM2INT`: through `to_int`, and a TypeError for anything that
    # has none or answers something other than an Integer.
    unless number.is_a?(Integer)
      unless number.respond_to?(:to_int)
        raise TypeError, "no implicit conversion of #{number.nil? ? "nil" : number.class} into Integer"
      end
      converted = number.to_int
      unless converted.is_a?(Integer)
        raise TypeError, "can't convert #{number.class} to Integer (#{number.class}#to_int gives #{converted.class})"
      end
      number = converted
    end
    __signal_list__[0].each { |name, known| return name if known == number }
    nil
  end
end

# `SignalException.new` takes a signal number, optionally with a message, or a
# signal name as a String or Symbol, with or without its `SIG` prefix. Every
# rule below is CRuby's `esignal_init`, measured against ruby 4.0.7.
class SignalException
  def initialize(*given)
    if given.empty?
      raise ArgumentError, "wrong number of arguments (given 0, expected 1)"
    end
    first = given[0]
    if first.is_a?(Integer) || (!first.is_a?(String) && !first.is_a?(Symbol) && first.respond_to?(:to_int))
      if given.size > 2
        raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 1..2)"
      end
      signo = first.is_a?(Integer) ? first : first.to_int
      table, limit = __signal_list__
      # Up to and including NSIG, measured: 65 is "SIG65" on Linux, 66 raises.
      if signo < 0 || signo > limit
        raise ArgumentError, "invalid signal number (#{signo})"
      end
      message = given.size > 1 ? given[1] : __signo_to_signm__(table, signo)
    else
      if given.size > 1
        raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 1)"
      end
      name = first.is_a?(Symbol) ? first.to_s : first
      unless name.is_a?(String)
        raise ArgumentError, "bad signal type #{first.class}"
      end
      bare = name.start_with?("SIG") ? name[3, name.length - 3] : name
      signo = nil
      __signal_list__[0].each { |known, number| signo = number if signo.nil? && known == bare }
      raise ArgumentError, "unsupported signal 'SIG#{bare}'" if signo.nil?
      # The name as given, not the table's: `SignalException.new("IOT")` is
      # "SIGIOT" even though `Signal.signame(6)` is "ABRT".
      message = "SIG" + bare
    end
    super(message)
    @signo = signo
  end

  def __signo_to_signm__(table, signo)
    table.each { |name, number| return "SIG" + name if number == signo }
    "SIG#{signo}"
  end

  def signo
    @signo
  end

  def signm
    message
  end
end

# `Interrupt` is `SignalException` for SIGINT, with "Interrupt" as its default
# message rather than "SIGINT".
class Interrupt
  def initialize(*given)
    if given.size > 1
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..1)"
    end
    super(Signal.list["INT"], given.empty? ? "Interrupt" : given[0])
  end
end

module Kernel
  def trap(signal, command = nil, &block)
    Signal.trap(signal, command, &block)
  end

  module_function :trap
end
