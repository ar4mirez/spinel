# Binding and string eval (#38).
#
# A Binding is a snapshot of a frame: its locals, `self`, lexical scope and
# block. `Kernel#binding` makes one natively — only the VM can see a frame —
# and the primitives below read and write it. `eval` compiles its string
# against the binding's local names and runs it in an environment inside the
# binding's, so locals the string assigns are the caller's and new ones are
# kept for the next `eval` through the same binding.

class Binding
  class << self
    undef_method :new
    undef_method :allocate
  end

  def eval(source, file = nil, line = nil)
    source = Kernel.__eval_source__(source)
    file = Kernel.__eval_file__(file, __caller_binding__)
    __binding_eval__(source, file, Kernel.__eval_line__(line))
  end

  def local_variable_get(name)
    name = __local_name__(name)
    found = __binding_get__(name)
    unless found
      raise NameError.new("local variable '#{name}' is not defined for #{inspect}", name)
    end
    found[0]
  end

  def local_variable_set(name, value)
    __binding_set__(__local_name__(name), value)
  end

  def local_variable_defined?(name)
    !__binding_get__(__local_name__(name)).nil?
  end

  def local_variables
    __binding_names__
  end

  def receiver
    __binding_receiver__
  end

  def source_location
    __binding_location__
  end

  # A Symbol, or a String naming one, that could be a local variable.
  def __local_name__(name)
    unless Symbol === name
      unless String === name
        raise TypeError, "#{name.inspect} is not a symbol nor a string" unless name.respond_to?(:to_str)
        name = name.to_str
      end
      name = name.to_sym
    end
    text = name.to_s
    if text.bytesize == 2 && text.getbyte(0) == 95 && text.getbyte(1) >= 49 && text.getbyte(1) <= 57
      raise NameError.new("numbered parameter '#{text}' is not a local variable", name)
    end
    first = text.empty? ? 0 : text.getbyte(0)
    valid = first == 95 || (first >= 97 && first <= 122) || first >= 128
    if valid
      text.each_byte do |byte|
        word = byte == 95 || (byte >= 48 && byte <= 57) || (byte >= 65 && byte <= 90) ||
               (byte >= 97 && byte <= 122) || byte >= 128
        valid = false unless word
      end
    end
    raise NameError.new("wrong local variable name '#{text}' for #{inspect}", name) unless valid
    name
  end
  private :__local_name__
end

module Kernel
  def eval(source, binding = nil, file = nil, line = nil)
    source = Kernel.__eval_source__(source)
    # `self` is this call's receiver: the caller's own, or `Kernel` for
    # `Kernel.eval`.
    caller = __caller_binding__.__binding_receiver_set__(self)
    unless binding.nil? || Binding === binding
      raise TypeError, "wrong argument type #{binding.class} (expected binding)"
    end
    file = Kernel.__eval_file__(file, caller) if binding || file
    (binding || caller).__binding_eval__(source, file, Kernel.__eval_line__(line))
  end

  def local_variables
    __caller_binding__.local_variables
  end

  module_function :eval, :local_variables, :binding

  # `eval`'s source: a String, or what `to_str` makes of anything else.
  def self.__eval_source__(source)
    return source if String === source
    unless source.respond_to?(:to_str)
      raise TypeError, "no implicit conversion of #{source.nil? ? "nil" : source.class} into String"
    end
    source.to_str
  end

  # The file an `eval` reports: the one it was given, or CRuby's
  # `(eval at FILE:LINE)` naming where the `eval` was written.
  def self.__eval_file__(file, caller)
    return __eval_source__(file) unless file.nil?
    path, line = caller.source_location
    path ? "(eval at #{path}:#{line})" : "(eval)"
  end

  def self.__eval_line__(line)
    return 1 if line.nil?
    Integer === line ? line : line.to_int
  end
end
