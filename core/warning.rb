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

  # `Regexp.new("a", Object.new)`: `Regexp#initialize` asks this to say what
  # it made of the second argument.
  def __regexp_flag_warning__(flag)
    __warning__("expected true or false as ignorecase: #{flag.inspect}", true)
  end

  # What the parser said about a file or an `eval` string, already a whole
  # line; only whether to print it is decided here.
  def __parse_warning__(text, verbose)
    return nil if $VERBOSE.nil? || (verbose && !$VERBOSE)
    __warning_send__(text, nil)
  end
  # "the block passed to 'm' ... may be ignored" (#273), already a whole line,
  # owed by a call that passed a block to a method that never uses one. It
  # prints under `-w`, or whenever `Warning[:strict_unused_block]` is on —
  # which also drops the relief of another `m` that does use its block.
  def __unused_block_warning__(text, relieved)
    return nil if $VERBOSE.nil?
    return nil unless Warning[:strict_unused_block] || ($VERBOSE && !relieved)
    __warning_send__(text, nil)
  end
  private :__warning__, :__warning_send__, :__regexp_flag_warning__, :__parse_warning__,
          :__unused_block_warning__
end

# The globals whose assignment is more than a store (#268). The VM sends an
# assignment to one of these here, and a read of `$=` to `__global_read__`;
# `__global_store__` is the cell itself.
#
# `name` is the global the hook was installed on and `as` the one the program
# wrote, which differ through an alias: `alias $NR $.` assigns by `$.`'s rule,
# and CRuby's messages name the alias — "value of $x must be String" after
# `alias $x $,`. Measured on 4.0.9 (#789).
module Kernel
  def __global_assign__(name, value, as = name)
    case name
    when :$VERBOSE, :$-v, :$-w
      # nil is "no warnings at all"; anything else is a boolean.
      value = value ? true : false unless value.nil?
    when :$,, :$/, :$-0, :$\
      unless value.nil? || value.is_a?(String)
        raise TypeError, "value of #{as} must be String"
      end
      __warning__("non-nil '#{as}' is deprecated", false, :deprecated) unless value.nil?
      # CRuby's `$/` setter keeps a frozen copy with no class or instance
      # variables of its own, or the default separator itself (#484).
      if (name == :$/ || name == :$-0) && !value.nil?
        if value == IO.__default_rs__
          value = IO.__default_rs__
        elsif !(value.frozen? && value.instance_of?(String) && value.instance_variables.empty?)
          value = String.new(value).freeze
        end
      end
    when :$;
      unless value.nil? || value.is_a?(String) || value.is_a?(Regexp)
        raise TypeError, "value of #{as} must be String or Regexp"
      end
      __warning__("non-nil '#{as}' is deprecated", false, :deprecated) unless value.nil?
    when :$=
      __warning__("variable $= is no longer effective; ignored", false, :deprecated)
      return nil
    when :$0, :$PROGRAM_NAME
      # A frozen copy, which is also what `ps` shows from now on: CRuby's
      # `set_arg0`. Measured on 4.0.9.
      unless String === value
        unless value.respond_to?(:to_str)
          raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into String"
        end
        value = value.to_str
      end
      value = value.dup.freeze
      __sys_proctitle__(value)
      __global_store__(:$0, value)
      return value
    when :$stdout, :$stderr, :$>
      # CRuby's `stdout_setter`: anything that answers `write`. Measured on
      # 4.0.9: "$stdout must have write method, NilClass given" (#789).
      unless value.respond_to?(:write)
        raise TypeError, "#{as} must have write method, #{value.class} given"
      end
    when :$.
      # `NUM2LONG`: a Float truncates, anything else goes through `to_int`.
      # Measured on 4.0.9: `$. = 123.5` reads back 123, and a `to_int` that
      # answers a String is "can't convert Object to Integer (...)" (#789).
      value = value.is_a?(Float) ? Array.__long__(value) : Integer.__c_long__(value)
    when :$@
      # `$@ = x` sets the backtrace of `$!`, so with no `$!` there is nothing
      # to set. Measured on 4.0.9.
      raise ArgumentError, "$! not set" if $!.nil?
      # ponytail: the value is kept in the cell, and `$@` still reads
      # `$!.backtrace`, so the assignment is not seen yet. Making it
      # `$!.set_backtrace(value)` is #29's, with the examples tagged for it.
    end
    __global_store__(name, value)
  end

  def __global_read__(name, as = name)
    case name
    when :$=
      __warning__("variable $= is no longer effective", false, :deprecated)
      false
    # Only an alias reads these here — `$!` and `$@` themselves compile to
    # the handler's exception — so `$ERROR_INFO` follows it as they do (#789).
    when :$!
      $!
    when :$@
      $@
    end
  end
  private :__global_assign__, :__global_read__
end

__hook_global__(false, :$VERBOSE, :$-v, :$-w, :$,, :$/, :$-0, :$\, :$;, :$stdout, :$stderr, :$>, :$.)
__hook_global__(true, :$=, :$!, :$@)
# `$!` is read-only; its cell holds nil only so that an alias of it reads as
# defined, as `$!` itself does. `$<` and `$FILENAME` belong to ARGF (#41) and
# are read-only whatever they hold; they are defined from the start too. `$.`
# counts lines from 0. All measured on 4.0.9 (#789).
__global_store__(:$!, nil)
__global_store__(:$@, nil)
__global_store__(:$<, nil)
__global_store__(:$FILENAME, nil)
__freeze_global__(:$!, :$<, :$FILENAME)
__global_store__(:$., 0)
