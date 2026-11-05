# `objspace` (#690): CRuby's `ext/objspace`, which asks the collector what
# `ObjectSpace` itself does not. Its C half is the `__objspace_*__`
# primitives over `heap.rs` and, where Ruby can say it, the JSON writer
# `_dump` and `_dump_all` below; its Ruby half, `dump` and `dump_all`, is
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
    out << _dump_object(obj, _dump_readers)
    out
  end

  # `full` asks for the empty slots too, which this heap does not walk.
  # What the walk allocates is made after the heap is listed, so the dump
  # does not describe itself.
  def self._dump_all(out, full, since, shapes)
    objects = __each_object__(nil)
    readers = _dump_readers
    json = +''
    objects.each do |obj|
      if Integer === since && since >= 0
        site = __objspace_allocation__(obj)
        next unless site && site[4] >= since
      end
      json << _dump_object(obj, readers) << "\n"
    end
    out << json
    out
  end

  # The core methods `_dump_object` reads an object through, bound to it
  # rather than sent: it may be a BasicObject, or define its own.
  def self._dump_readers
    {
      frozen: Kernel.instance_method(:frozen?),
      bytes: String.instance_method(:b),
      encoding: String.instance_method(:encoding),
      length: Array.instance_method(:length),
      name: Module.instance_method(:__name__),
      float: Float.instance_method(:to_s),
    }
  end

  # `obj`'s line of `dump`: CRuby's keys, for what this heap can say about
  # it. No `shape_id`, `slot_size` or `flags`: those describe CRuby's own
  # heap, and a program reading them reads CRuby.
  def self._dump_object(obj, readers)
    address = __objspace_address__(obj, false)
    if address.nil?
      return "null" if obj.nil?
      return %({"type":"SYMBOL", "value":#{_json_string(obj.name)}}) if Symbol === obj
      return obj.to_s
    end
    type = __objspace_type__(obj).name[2..]
    line = +%({"address":"0x#{address.to_s(16)}", "type":"#{type}")
    klass = __objspace_address__(obj, true)
    line << %(, "class":"0x#{klass.to_s(16)}") if klass
    line << ', "frozen":true' if readers[:frozen].bind_call(obj)
    case type
    when "STRING"
      bytes = readers[:bytes].bind_call(obj)
      encoding = readers[:encoding].bind_call(obj).name
      line << %(, "bytesize":#{bytes.bytesize}, "value":#{_json_string(bytes)})
      line << %(, "encoding":#{_json_string(encoding)})
    when "ARRAY"
      line << %(, "length":#{readers[:length].bind_call(obj)})
    when "CLASS", "MODULE"
      name = readers[:name].bind_call(obj)
      line << %(, "name":#{_json_string(name)}) if name
    when "FLOAT"
      line << %(, "value":"#{readers[:float].bind_call(obj)}")
    end
    references = __objspace_reachable__(obj)
    unless references.empty?
      addresses = references.map { |ref| %("0x#{__objspace_address__(ref, false).to_s(16)}") }
      line << %(, "references":[#{addresses.join(", ")}])
    end
    site = __objspace_allocation__(obj)
    if site
      line << %(, "file":#{_json_string(site[0])}, "line":#{site[1]})
      line << %(, "method":#{_json_string(site[3].name)}) if site[3]
      line << %(, "generation":#{site[4]})
    end
    line << %(, "memsize":#{__objspace_memsize__(obj)}})
  end

  # `bytes` as a JSON string, escaped as CRuby's `objspace` escapes one,
  # with what is not UTF-8 replaced. Each character to escape is split on
  # and joined back escaped, the backslash first, since the escapes are
  # written with one.
  def self._json_string(bytes)
    text = bytes.b.force_encoding(Encoding::UTF_8).scrub("�")
    return %("#{text}") unless text.match?(/["\\\x00-\x1f\x7f]/)
    text = text.split("\\", -1).join("\\\\") if text.include?("\\")
    text = text.split('"', -1).join('\\"') if text.include?('"')
    if text.match?(/[\x00-\x1f\x7f]/)
      [*0..0x1f, 0x7f].each do |code|
        ch = code.chr
        next unless text.include?(ch)
        escape = case ch
                 when "\n" then "\\n"
                 when "\r" then "\\r"
                 when "\t" then "\\t"
                 when "\b" then "\\b"
                 when "\f" then "\\f"
                 else "\\u00" + (code < 0x10 ? "0" : "") + code.to_s(16)
                 end
        text = text.split(ch, -1).join(escape)
      end
    end
    %("#{text}")
  end

  def self._dump_shapes(out, since)
    out
  end

  class << self
    private :_dump
    private :_dump_all
    private :_dump_readers
    private :_dump_object
    private :_json_string
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
