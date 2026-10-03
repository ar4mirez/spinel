# Signal (#29).
#
# The table is the platform's and comes from `__signal_list__`: CRuby's
# `siglist` order, with this platform's numbers from the `libc` crate. Only the
# two methods that read it are here; `trap` and delivery are Phase 3.
module Signal
  # A new Hash every call, as in CRuby, so a caller that mutates it cannot
  # change what the next caller sees.
  def self.list
    table = {}
    __signal_list__[0].each { |name, number| table[name] = number }
    table
  end

  # ponytail: a handler is recorded, answered back by the next `trap`, and
  # never run: delivering a signal means an OS handler and a safe point in
  # the interpreter loop, which is `Process` (#43) along with `Process.kill`.
  #
  # What `trap` answers is CRuby's, measured on 4.0.7, by signal number so an
  # alias shares its handler: the signals Ruby reserves start "DEFAULT",
  # SIGPIPE is ignored, ABRT and SYS hold a C handler (nil), and the rest
  # start at the operating system's "SYSTEM_DEFAULT".
  RESERVED = %w[HUP INT QUIT ALRM TERM CHLD USR1 USR2].freeze
  
  def self.trap(signal, command = nil, &block)
    name =
      case signal
      when Integer then signal == 0 ? "EXIT" : (signame(signal) || raise(ArgumentError, "invalid signal number (#{signal})"))
      when Symbol, String then signal.to_s.delete_prefix("SIG")
      else raise ArgumentError, "bad signal type #{signal.class}"
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
    handlers = (@__handlers__ ||= {})
    previous = handlers.key?(number) ? handlers[number] : __initial_handler__(number)
    handlers[number] = handler
    previous
  end
  
  def self.__initial_handler__(number)
    name = signame(number)
    return nil if number == 0 || name == "ABRT" || name == "SYS"
    return "IGNORE" if name == "PIPE"
    RESERVED.include?(name) ? "DEFAULT" : "SYSTEM_DEFAULT"
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
