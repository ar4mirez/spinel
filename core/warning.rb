# Warning — where every warning goes on its way to `$stderr`.
#
# `Kernel#warn`, the parser, the VM and the core library all end in
# `Warning.warn(text)`, so a program that redefines it sees every one of them
# (#268). The text is already whole by then: position, `warning:` and the
# newline are the sender's business, and this module only decides whether a
# category is switched on and writes.
module Warning
  # ruby 4.0.6's `Warning.categories`, and what each starts as.
  @categories = {
    deprecated: false,
    experimental: true,
    performance: false,
    strict_unused_block: false
  }

  def self.[](category)
    __category__(category)
    @categories[category]
  end

  def self.[]=(category, value)
    __category__(category)
    @categories[category] = value ? true : false
    value
  end

  def self.categories
    @categories.keys
  end

  def self.__category__(category)
    unless category.is_a?(Symbol)
      raise TypeError, "wrong argument type #{category.class} (expected Symbol)"
    end
    raise ArgumentError, "unknown category: #{category}" unless @categories.key?(category)
  end

  # `*rest` is there for the arity, not for arguments: `Kernel#warn` passes
  # `category:` only to a `warn` that is not exactly one-argument, which is how
  # CRuby tells a program's `def warn(message)` from this one.
  def warn(message, *rest, category: nil)
    unless rest.empty?
      raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 1)"
    end
    unless message.is_a?(String)
      raise TypeError, "wrong argument type #{message.class} (expected String)"
    end
    unless message.encoding.ascii_compatible?
      raise Encoding::CompatibilityError, "ASCII incompatible encoding: #{message.encoding}"
    end
    unless category.nil?
      category = category.to_sym unless category.is_a?(Symbol)
      return nil unless Warning[category]
    end
    $stderr.write(message)
    nil
  end

  extend self
end

module Kernel
  # A warning from the core library or the VM, as CRuby's `rb_warn` and
  # `rb_warning` print one: `file:line: warning: message`, at the line of the
  # program that called in. `verbose` is `rb_warning` — only under `-w` — and
  # a `category` that is switched off says nothing.
  def __warning__(message, verbose = false, category = nil)
    return nil if $VERBOSE.nil? || (verbose && !$VERBOSE)
    return nil if !category.nil? && !Warning[category]
    here = __backtrace_here__[0]
    text = here.nil? ? "warning: #{message}\n" : "#{here[0]}:#{here[1]}: warning: #{message}\n"
    __warning_send__(text, category)
  end

  # `Warning.warn(text)`, with `category:` unless the method there takes
  # exactly one argument.
  def __warning_send__(text, category)
    if __reflect_method_arity__(Warning, :warn) == 1
      Warning.warn(text)
    else
      Warning.warn(text, category: category)
    end
    nil
  end

  # `Regexp.new("a", Object.new)`: the VM builds the pattern and asks this to
  # say what it made of the second argument.
  def __regexp_flag_warning__(flag)
    __warning__("expected true or false as ignorecase: #{flag.inspect}", true)
  end

  # What the parser said about a file or an `eval` string, already a whole
  # line; only whether to print it is decided here.
  def __parse_warning__(text, verbose)
    return nil if $VERBOSE.nil? || (verbose && !$VERBOSE)
    __warning_send__(text, nil)
  end
  private :__warning__, :__warning_send__, :__regexp_flag_warning__, :__parse_warning__
end

# The globals whose assignment is more than a store (#268). The VM sends an
# assignment to one of these here, and a read of `$=` to `__global_read__`;
# `__global_store__` is the cell itself.
module Kernel
  def __global_assign__(name, value)
    case name
    when :$VERBOSE, :$-v, :$-w
      # nil is "no warnings at all"; anything else is a boolean.
      value = value ? true : false unless value.nil?
    when :$,, :$/, :$-0, :$\
      unless value.nil? || value.is_a?(String)
        raise TypeError, "value of #{name} must be String"
      end
      __warning__("non-nil '#{name}' is deprecated", false, :deprecated) unless value.nil?
    when :$;
      unless value.nil? || value.is_a?(String) || value.is_a?(Regexp)
        raise TypeError, "value of $; must be String or Regexp"
      end
      __warning__("non-nil '$;' is deprecated", false, :deprecated) unless value.nil?
    when :$=
      __warning__("variable $= is no longer effective; ignored", false, :deprecated)
      return nil
    end
    __global_store__(name, value)
  end

  def __global_read__(name)
    __warning__("variable $= is no longer effective", false, :deprecated) if name == :$=
    false
  end
  private :__global_assign__, :__global_read__
end

__hook_global__(false, :$VERBOSE, :$-v, :$-w, :$,, :$/, :$-0, :$\, :$;)
__hook_global__(true, :$=)
