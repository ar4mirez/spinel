# Exception.
#
# The VM writes `@message` when it raises, so everything here is ordinary Ruby
# reading an ordinary instance variable. It was two primitives over two fixed
# slots until #151 gave objects a shape to hold ivars in.
#
# `backtrace` is always nil: a real one needs source positions the compiler does
# not record, and `[]` would be a plausible-but-wrong answer rather than an
# absent one. `full_message` and `detailed_message` wait on the same thing,
# which PRD 0012 named as a non-goal.
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

  def exception(text = nil)
    return self if text.nil?
    self.class.new(text)
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Exception)
    self.class.equal?(other.class) && message == other.message
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
