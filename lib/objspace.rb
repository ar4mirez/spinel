# `objspace` (#690): CRuby's `ext/objspace`, which asks the collector what
# `ObjectSpace` itself does not. Its C half is the `__objspace_*__`
# primitives over `heap.rs`; its Ruby half, `dump` and `dump_all` below, is
# upstream's `objspace.rb`.

module ObjectSpace
  # The bytes `obj` occupies: its cell, and the storage it owns. An
  # immediate occupies none.
  def self.memsize_of(obj)
    __objspace_memsize__(obj)
  end

  def self.memsize_of_all(klass = nil)
    unless klass.nil? || __reflect_module_kind__(klass)
      raise TypeError, "class or module required"
    end
    total = 0
    __each_object__(klass || BasicObject).each { |obj| total += __objspace_memsize__(obj) }
    total
  end

  # CRuby's type names, `:T_STRING` and the rest, for what `dump` calls each
  # object, with `:TOTAL` their sum.
  def self.count_objects_size(result = {})
    raise TypeError, "non-hash given" unless Hash === result
    total = 0
    __each_object__(BasicObject).each do |obj|
      size = __objspace_memsize__(obj)
      type = __objspace_type__(obj)
      result[type] = (result[type] || 0) + size
      total += size
    end
    result[:TOTAL] = total
    result
  end

  def self.reachable_objects_from(obj)
    __objspace_reachable__(obj)
  end

  def self.trace_object_allocations_start
    __objspace_trace__(true)
    nil
  end

  def self.trace_object_allocations_stop
    __objspace_trace__(false)
    nil
  end

  def self.trace_object_allocations_clear
    __objspace_trace__(nil)
    nil
  end

  def self.trace_object_allocations
    trace_object_allocations_start
    begin
      yield
    ensure
      trace_object_allocations_stop
    end
  end

  # ponytail: CRuby's debug mode prints each allocation site from a signal
  # handler. Nothing here reads it yet, so it only answers.
  def self.trace_object_allocations_debug_start
    trace_object_allocations_start
  end

  def self.allocation_sourcefile(obj)
    site = __objspace_allocation__(obj)
    site && site[0]
  end

  def self.allocation_sourceline(obj)
    site = __objspace_allocation__(obj)
    site && site[1]
  end

  def self.allocation_class_path(obj)
    site = __objspace_allocation__(obj)
    site && site[2]
  end

  def self.allocation_method_id(obj)
    site = __objspace_allocation__(obj)
    site && site[3]
  end

  def self.allocation_generation(obj)
    site = __objspace_allocation__(obj)
    site && site[4]
  end

  def self._dump(obj, out)
    out << __objspace_dump__(obj)
    out
  end

  # `full` asks for the empty slots too, which this heap does not walk.
  def self._dump_all(out, full, since, shapes)
    out << __objspace_dump_all__(since)
    out
  end

  def self._dump_shapes(out, since)
    out
  end

  class << self
    private :_dump
    private :_dump_all
    private :_dump_shapes
  end

  module_function

  # Dump the contents of a ruby object as JSON.
  #
  # _output_ can be one of: +:stdout+, +:file+, +:string+, or IO object.
  def dump(obj, output: :string)
    out = case output
    when :file, nil
      require 'tempfile'
      Tempfile.create(%w(rubyobj .json))
    when :stdout
      STDOUT
    when :string
      +''
    when IO
      output
    else
      raise ArgumentError, "wrong output option: #{output.inspect}"
    end

    ret = _dump(obj, out)
    return nil if output == :stdout
    ret
  end

  # Dump the contents of the ruby heap as JSON.
  #
  # _output_ argument is the same as for #dump.
  def dump_all(output: :file, full: false, since: nil, shapes: true)
    out = case output
    when :file, nil
      require 'tempfile'
      Tempfile.create(%w(rubyheap .json))
    when :stdout
      STDOUT
    when :string
      +''
    when IO
      output
    else
      raise ArgumentError, "wrong output option: #{output.inspect}"
    end

    shapes = 0 if shapes == true
    ret = _dump_all(out, full, since, shapes)
    return nil if output == :stdout
    ret
  end

  # Dump the contents of the ruby shape tree as JSON.
  def dump_shapes(output: :file, since: 0)
    out = case output
    when :file, nil
      require 'tempfile'
      Tempfile.create(%w(rubyshapes .json))
    when :stdout
      STDOUT
    when :string
      +''
    when IO
      output
    else
      raise ArgumentError, "wrong output option: #{output.inspect}"
    end

    ret = _dump_shapes(out, since)
    return nil if output == :stdout
    ret
  end
end
