# Exception.
#
# The VM writes `@message` when it raises, so everything here is ordinary Ruby
# reading an ordinary instance variable. It was two primitives over two fixed
# slots until #151 gave objects a shape to hold ivars in.
#
# `backtrace` is `@backtrace`, which the VM fills in when the exception is
# first raised (#29): the compiler records a line per instruction, and the
# frame stack at the raise is the backtrace. `full_message`, `cause` and the
# rest are at the end of this file.
class Exception
  # `message` is `to_s`, not `@message`: a subclass that overrides `to_s` changes
  # what `message` and `inspect` answer, which is what ruby/spec pins.
  def message
    to_s
  end

  def to_s
    @message
  end

  def backtrace
    @backtrace
  end

  def inspect
    text = to_s
    text.empty? ? self.class.name : "#<" + self.class.name + ": " + text + ">"
  end

  # No argument, or the exception itself, is the exception itself — CRuby's
  # `exc_exception` checks identity before anything else. Anything else, nil
  # included, is a new one of the same class carrying that message: measured,
  # `e.exception(nil)` is a different object whose message is the class name.
  def exception(*given)
    if given.size > 1
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..1)"
    end
    return self if given.empty? || given[0].equal?(self)
    # A copy carrying the new message, not a new exception: CRuby clones and
    # sets the message without running `initialize` again, so state the
    # constructor set — `CustomArgumentError#val` — survives. Measured.
    copy = clone
    copy.__send__(:__replace_message__, given[0])
    copy
  end

  def __replace_message__(text)
    @message = text
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Exception)
    self.class.equal?(other.class) && message == other.message && backtrace == other.backtrace
  end
end

# NameError, and NoMethodError under it.
#
# The VM writes `@name` and `@receiver` when a dispatch fails, the same way it
# writes `@message` when it raises, so these are two more ordinary readers over
# ordinary instance variables.
#
# `NameError.new` is refused — `exceptions.txt` marks it `# own initialize` —
# so every NameError that exists is one the VM built, and there is no
# user-constructed one whose `receiver` should be CRuby's ArgumentError.
class NameError
  def name
    @name
  end

  def receiver
    @receiver
  end
end

# `Exception#initialize` is Ruby now, which is what lets the subclasses below
# have their own and reach this through `super`. The VM still writes `@message`
# directly on the path where *it* raises; this is the path where a program
# builds one with `new`.
#
# Measured: `StandardError.new.message` is "StandardError", so an absent
# message is the class name rather than nil, and a non-String is converted.
class Exception
  def initialize(message = nil)
    @message = message.nil? ? self.class.name : message.to_s
  end
end

# `SystemExit` carries an exit status beside its message, and the two arguments
# are positional-but-either: `SystemExit.new(1)`, `SystemExit.new("m")` and
# `SystemExit.new(2, "m")` are all valid. A `true` status is 0 and a `false`
# one is 1 — the shell convention, inverted from Ruby's truthiness.
class SystemExit
  def initialize(*given)
    status = 0
    message = nil
    unless given.empty?
      first = given[0]
      if first.is_a?(String)
        message = first
      else
        status = __status_from__(first)
        message = given[1]
      end
    end
    super(message)
    @status = status
  end

  def __status_from__(value)
    return 0 if value == true
    return 1 if value == false
    value
  end

  def status
    @status
  end

  def success?
    @status == 0
  end
end

# `NameError` takes the name as a second positional and the receiver as a
# keyword. `receiver` on one built without it is an ArgumentError rather than
# nil: "no receiver is available" — measured, and the reason `@receiver` cannot
# simply default to nil.
class NameError
  def initialize(message = nil, name = nil, receiver: __no_receiver__)
    super(message)
    @name = name
    @receiver = receiver
  end

  def __no_receiver__
    :__spinel_no_receiver__
  end

  def receiver
    if @receiver == :__spinel_no_receiver__
      raise ArgumentError, "no receiver is available"
    end
    @receiver
  end
end

# `NoMethodError` adds the call's arguments and whether it was a private call.
class NoMethodError
  def initialize(message = nil, name = nil, args = nil, private_call = false, receiver: __no_receiver__)
    super(message, name, receiver: receiver)
    @args = args
    @private_call = private_call
  end

  def args
    @args
  end

  def private_call?
    @private_call
  end
end

# `FrozenError` carries the object that was frozen, under the same
# absent-is-an-error rule as `NameError#receiver`.
class FrozenError
  def initialize(message = nil, receiver: __no_receiver__)
    super(message)
    @receiver = receiver
  end

  def __no_receiver__
    :__spinel_no_receiver__
  end

  def receiver
    if @receiver == :__spinel_no_receiver__
      raise ArgumentError, "no receiver is available"
    end
    @receiver
  end
end

# `KeyError` carries the hash and the key that was missing. Both are keywords,
# and both raise when absent rather than answering nil.
class KeyError
  def initialize(message = nil, receiver: __no_receiver__, key: __no_receiver__)
    super(message)
    @receiver = receiver
    @key = key
  end

  def __no_receiver__
    :__spinel_no_receiver__
  end

  def receiver
    raise ArgumentError, "no receiver is available" if @receiver == :__spinel_no_receiver__
    @receiver
  end

  def key
    raise ArgumentError, "no key is available" if @key == :__spinel_no_receiver__
    @key
  end
end

# `NoMatchingPatternKeyError` is the pattern-matching sibling: `matchee` is the
# value that did not match and `key` is the key that was missing from it.
class NoMatchingPatternKeyError
  def initialize(message = nil, matchee: nil, key: nil)
    super(message)
    @matchee = matchee
    @key = key
  end

  def matchee
    @matchee
  end

  def key
    @key
  end
end

# `StopIteration#result` is what the finished enumerator's `each` returned.
class StopIteration
  def result
    @result
  end
end

# `SyntaxError` defaults its message to "compile error" rather than to the class
# name, which is the one place `Exception#initialize`'s rule does not hold.
# `path` is the file the error was found in, and nil for one built directly
# rather than raised by the parser.
class SyntaxError
  def initialize(*given)
    super(given.empty? || given[0].nil? ? "compile error" : given[0])
  end

  def path
    @path
  end
end

# `SystemCallError` and the `Errno::E*` classes under it (#29).
#
# The classes, their `Errno` constants and the number-to-class map are the
# platform's and come from bootstrap; the default message is `strerror(3)`
# through `__strerror__`. Everything else is here.
#
# Called on `SystemCallError` itself the arguments are `(message, errno, func)`
# and the answer is an instance of the `Errno` class that number belongs to —
# `SystemCallError.new("m", 2)` is an `Errno::ENOENT`. CRuby does that by
# changing the new object's class in the middle of `initialize`, which Ruby
# cannot, so `new` picks the class first. Called on an `Errno` class the
# arguments are `(message, func)` and the number is the class's own constant.
class SystemCallError
  def self.new(*given)
    return super unless equal?(SystemCallError)
    message, errno, func = __syserr_arguments__(given)
    target = errno.nil? ? nil : __errno_class__(__syserr_long__(errno))
    exception = (target || SystemCallError).allocate
    exception.__send__(:__syserr_setup__, message, errno, func)
    exception
  end

  def self.__syserr_arguments__(given)
    if given.empty? || given.size > 3
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 1..3)"
    end
    message, errno, func = given
    # A lone Integer is the errno, not the message: `SystemCallError.new(2)`.
    if given.size == 1 && message.is_a?(Integer)
      errno = message
      message = nil
    end
    [message, errno, func]
  end

  # CRuby's `NUM2LONG`: an Integer as is, anything else through `to_int` or a
  # TypeError. A Float truncates through `Float#to_int`, which is #18's.
  def self.__syserr_long__(value)
    return value if value.is_a?(Integer)
    unless value.respond_to?(:to_int)
      raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into Integer"
    end
    value.to_int
  end

  def initialize(*given)
    if self.class.equal?(SystemCallError)
      message, errno, func = SystemCallError.__syserr_arguments__(given)
    else
      if given.size > 2
        raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..2)"
      end
      message, func = given
      errno = self.class::Errno
    end
    __syserr_setup__(message, errno, func)
  end

  def __syserr_setup__(message, errno, func)
    text = errno.nil? ? "unknown error" : __strerror__(__syserr_int__(errno))
    # `func` only appears beside a message: `Errno::EINVAL.new(nil, "loc")` is
    # plain "Invalid argument". Measured.
    unless message.nil?
      unless message.is_a?(String)
        raise TypeError, "no implicit conversion of #{message.class} into String"
      end
      text = text + " @ " + func.to_s unless func.nil?
      text = text + " - " + message
    end
    @message = text
    @errno = errno
  end

  # CRuby's `NUM2INT`, which is what `strerror` is called through: in range for
  # a C `int` or a RangeError naming the value.
  def __syserr_int__(value)
    number = SystemCallError.__syserr_long__(value)
    if number > 2147483647
      raise RangeError, "integer #{number} too big to convert to 'int'"
    elsif number < -2147483648
      raise RangeError, "integer #{number} too small to convert to 'int'"
    end
    number
  end

  def errno
    @errno
  end
end

# Backtraces, `cause`, and the messages built from them (#29).
#
# The VM records a backtrace when an exception is first raised, as two ivars:
# `@backtrace`, the Strings, and `@__backtrace_locations__`, the same lines as
# `[path, line, label]`. `@__cause__` is the `$!` of that first raise. Every
# method here is Ruby over those three.
class Exception
  def self.to_tty?
    __stderr_tty__
  end

  def backtrace_locations
    raw = @__backtrace_locations__
    return nil if raw.nil?
    @__locations__ ||= raw.map { |path, line, label| Thread::Backtrace::Location.__from__(path, line, label) }
  end

  # An Array of Strings, one String, nil, or (Ruby 3.4) an Array of
  # `Thread::Backtrace::Location`s — the only shape that also sets
  # `backtrace_locations`. Anything else is a TypeError. Measured.
  def set_backtrace(backtrace)
    message = "backtrace must be an Array of String or an Array of Thread::Backtrace::Location"
    if backtrace.nil?
      @backtrace = nil
    elsif backtrace.is_a?(String)
      @backtrace = [backtrace]
    elsif backtrace.is_a?(Array)
      if !backtrace.empty? && backtrace.all? { |line| line.is_a?(Thread::Backtrace::Location) }
        @backtrace = backtrace.map(&:to_s)
        @__backtrace_locations__ = backtrace.map { |l| [l.path, l.lineno, l.label] }
        @__locations__ = nil
        return backtrace
      end
      raise TypeError, message unless backtrace.all? { |line| line.is_a?(String) }
      @backtrace = backtrace
    else
      raise TypeError, message
    end
    @__backtrace_locations__ = nil
    @__locations__ = nil
    backtrace
  end

  def cause
    @__cause__
  end

  # The message with the class name after its first line, which is what
  # `full_message` prints: "boom (RuntimeError)". Three exceptions to that,
  # all measured — an empty message is the class name alone ("unhandled
  # exception" for a bare RuntimeError), and an anonymous class adds nothing.
  def detailed_message(highlight: false, **)
    __check_highlight__(highlight)
    text = message
    text = text.nil? ? "" : text.to_s
    klass = self.class
    name = klass.name
    if text.empty?
      shown = klass.equal?(RuntimeError) ? "unhandled exception" : (name.nil? ? klass.inspect : name)
      return highlight ? "\e[1;4m" + shown + "\e[m" : shown
    end
    return text if name.nil?
    at = text.index("\n")
    first = at.nil? ? text : text[0, at]
    rest = at.nil? ? nil : text[at + 1, text.length - at - 1]
    unless highlight
      out = first + " (" + name + ")"
      out = out + "\n" + rest unless rest.nil?
      return out
    end
    out = "\e[1m" + first + " (\e[1;4m" + name + "\e[m\e[1m)\e[m"
    Exception.__each_line__(rest) { |line| out = out + "\n\e[1m" + line + "\e[m" } unless rest.nil?
    out
  end

  # What Ruby prints for an exception nobody rescued: the position, the
  # detailed message, then one "from" line per remaining frame — and then the
  # same for each `cause`, outermost first. `order: :bottom` reverses both and
  # numbers the frames. Every string here was measured against ruby 4.0.7.
  def full_message(highlight: __no_highlight__, order: :top, **options)
    highlight = Exception.to_tty? if highlight.equal?(__no_highlight__) || highlight.nil?
    __check_highlight__(highlight)
    order = :top if order.nil?
    unless order == :top || order == :bottom
      raise ArgumentError, "expected :top or :bottom as order: #{order.inspect}"
    end
    here = __backtrace_here__[0]
    fallback = here.nil? ? nil : "#{here[0]}:#{here[1]}:in 'full_message'"
    chain = []
    seen = []
    current = self
    until current.nil? || seen.any? { |e| e.equal?(current) }
      seen << current
      chain << current.__full_message_block__(highlight, order, fallback, options)
      current = current.respond_to?(:cause) ? current.cause : nil
    end
    if order == :top
      chain.join
    else
      title = highlight ? "\e[1mTraceback\e[m" : "Traceback"
      title + " (most recent call last):\n" + chain.reverse.join
    end
  end

  def __full_message_block__(highlight, order, fallback, options)
    detailed = respond_to?(:detailed_message) ? detailed_message(highlight: highlight, **options) : nil
    detailed = detailed.to_str unless detailed.nil? || detailed.is_a?(String)
    if detailed.nil?
      name = self.class.name || self.class.inspect
      detailed = highlight ? "\e[1;4m" + name + "\e[m" : name
    end
    lines = backtrace
    position = lines.nil? || lines.empty? ? fallback : lines[0]
    head = position.nil? ? detailed : position + ": " + detailed
    head = head + "\n" unless head.end_with?("\n")
    rest = lines.nil? ? [] : lines.__take__(1, lines.size - 1)
    if order == :top
      out = head
      rest.each { |line| out = out + "\tfrom " + line + "\n" }
      out
    else
      width = rest.size.to_s.length
      out = ""
      i = rest.size
      while i > 0
        number = i.to_s
        number = " " + number while number.length < width
        out = out + "\t" + number + ": from " + rest[i - 1] + "\n"
        i -= 1
      end
      out + head
    end
  end

  def __check_highlight__(highlight)
    return if highlight == true || highlight == false
    raise ArgumentError, "expected true or false as highlight: #{highlight.inspect}"
  end

  def __no_highlight__
    :__spinel_no_highlight__
  end

  def self.__each_line__(text)
    until text.empty?
      at = text.index("\n")
      if at.nil?
        yield text
        return
      end
      yield text[0, at]
      text = text[at + 1, text.length - at - 1]
    end
  end
end

# `caller` and `caller_locations` (#29): the frames above the caller's, as
# Strings or as `Thread::Backtrace::Location`s. `start` frames are skipped
# (default 1, the caller itself), then `length` are kept, or a Range picks
# them. More skipped than there are is nil; exactly as many is [].
module Kernel
  def caller(start = 1, length = nil)
    lines = __caller_select__(__backtrace_here__, start, length)
    lines && lines.map { |path, line, label| "#{path}:#{line}:in '#{label}'" }
  end

  def caller_locations(start = 1, length = nil)
    lines = __caller_select__(__backtrace_here__, start, length)
    lines && lines.map { |path, line, label| Thread::Backtrace::Location.__from__(path, line, label) }
  end

  def __caller_select__(lines, start, length)
    if start.is_a?(Range)
      raise TypeError, "no implicit conversion of Range into Integer" unless length.nil?
      first = start.begin.nil? ? 0 : __caller_int__(start.begin)
      last = start.end.nil? ? -1 : __caller_int__(start.end)
      last -= 1 if start.exclude_end? && !start.end.nil?
      raise ArgumentError, "negative level (#{first})" if first < 0
      return nil if first > lines.size
      last += lines.size if last < 0
      return [] if last < first
      return lines.__take__(first, last - first + 1)
    end
    first = __caller_int__(start)
    raise ArgumentError, "negative level (#{first})" if first < 0
    unless length.nil?
      length = __caller_int__(length)
      raise ArgumentError, "negative size (#{length})" if length < 0
    end
    return nil if first > lines.size
    kept = lines.__take__(first, lines.size - first)
    length.nil? ? kept : kept.__take__(0, length)
  end

  def __caller_int__(value)
    return value if value.is_a?(Integer)
    raise TypeError, "no implicit conversion of #{value.class} into Integer" unless value.respond_to?(:to_int)
    value.to_int
  end

  module_function :caller, :caller_locations
end
