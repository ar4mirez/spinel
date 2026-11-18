# frozen_string_literal: true

# `psych`'s C half on Spinel (#108). `require "psych"` is ruby/psych's own
# `psych.rb` (vendored in `stdlib/` at the release Ruby 4.0 bundles), which
# requires this in place of `psych.so`.
#
# What is here is `ext/psych` file by file: `psych.c`'s
# `Psych.libyaml_version`; `psych_parser.c`'s `Psych::Parser#_native_parse`
# and `#mark`; `psych_emitter.c`'s `Psych::Emitter`; `psych_to_ruby.c`'s
# `ToRuby#build_exception` and `ClassLoader#path2class`; and the classes
# `psych_yaml_tree.c` defines. The YAML itself is libyaml 0.2.5, the release
# CRuby's Psych links, in Rust (`spinel-vm/src/psych.rs`, the `__psych_*__`
# primitives), so events, marks, errors and emitted bytes are libyaml's.
#
# The parser runs over the whole input in one primitive call, which answers
# every event and the syntax error that stopped it, if one did; the events
# then go to the handler in order, each after `event_location`, as
# `psych_parser.c` calls them, and the error is raised after the last. A
# document nested deeper than 10,000 collections stops with a
# `Psych::SyntaxError` at the collection past it (`psych.rs`, `MAX_DEPTH`),
# where CRuby's Psych would parse on and raise `SystemStackError` building
# objects a few thousand levels down. Aliases are never expanded, here or
# by Psych's Ruby half: an alias loads as the object its anchor made, so a
# "billion laughs" document costs what its text does, and `Psych.load` and
# `safe_load` refuse aliases unless given `aliases: true`.
#
# ponytail: an IO is read whole before the first event goes to the handler,
# where `psych_parser.c` reads it 16 KiB at a time as libyaml asks, so a
# stream that never ends is never parsed, and a handler sees nothing of a
# long IO until all of it is read. Upgrade: keep a parser per
# `Psych::Parser` in the heap and feed it what each `read` answers.
#
# An emitter's libyaml state lives in the heap, under an id the object holds
# in a one-element Array, the holder, whose finalizer frees it.

require "psych/syntax_error"

module Psych
  # psych.c: the libyaml `spinel-vm/src/psych.rs` is.
  def self.libyaml_version = [0, 2, 5]

  # `rb_str_export_to_enc(string, UTF-8)`: `string` in UTF-8, or as it is
  # when it cannot be converted.
  def self.__export__(string, encoding = Encoding::UTF_8)
    return string if string.encoding == encoding
    return string if string.ascii_only? && string.encoding.ascii_compatible? && encoding.ascii_compatible?
    string.encode(encoding)
  rescue EncodingError
    string
  end

  # `Check_Type(value, T_STRING)`.
  def self.__check_string__(value)
    return value if String === value
    raise TypeError, "wrong argument type #{__type_name__(value)} (expected String)"
  end

  # `Check_Type(value, T_ARRAY)`.
  def self.__check_array__(value)
    return value if Array === value
    raise TypeError, "wrong argument type #{__type_name__(value)} (expected Array)"
  end

  def self.__type_name__(value)
    case value
    when nil then "nil"
    when true then "true"
    when false then "false"
    else value.class.to_s
    end
  end

  # `StringValueCStr`: a String that is to go to libyaml as a C string.
  def self.__cstr__(string)
    raise ArgumentError, "string contains null byte" if string.include?("\0")
    string
  end

  # A holder for emitter `id`: the Array the emitter keeps, with the
  # finalizer that frees the libyaml emitter once nothing keeps the holder.
  #
  # ponytail: registered with `__finalizer_add__`, as `openssl`'s are, not
  # through `ObjectSpace.define_finalizer`, whose capture check keeps memory
  # for every call (lib/openssl/spinel.rb has the upgrade).
  def self.__holder__(id)
    holder = [id]
    __finalizer_add__(holder, __free__(id))
    holder
  end

  # Made here, so the finalizer holds the id and not the holder.
  def self.__free__(id) = proc { __psych_emitter_free__(id) }

  private_class_method :__export__, :__check_string__, :__check_array__, :__type_name__,
    :__cstr__, :__holder__, :__free__

  # psych_emitter.c's superclass, which psych/handler.rb fills in.
  class Handler
  end

  # psych_to_ruby.c and psych_yaml_tree.c define these, empty, for the
  # methods below and psych/visitors to reopen.
  module Visitors
    class Visitor
    end

    class ToRuby < Visitor
      private

      # Create an exception with class +klass+ and +message+, without
      # calling its +initialize+.
      def build_exception(klass, message)
        exception = klass.allocate
        exception.instance_variable_set(:@__message__, message)
        exception
      end
    end

    class YAMLTree < Visitor
    end
  end

  class ClassLoader
    private

    # Convert +path+ string to a class: `rb_path_to_class`.
    def path2class(path)
      path = Psych.__send__(:__check_string__, path) unless String === path
      if path.start_with?("#")
        raise ArgumentError, "can't retrieve anonymous class #{path}"
      end
      mod = Object
      offset = 0
      loop do
        stop = path.index("::", offset)
        name = path[offset...(stop || path.length)]
        # The path through this name's `::`, as CRuby's message has it.
        so_far = stop ? path[0, stop + 2] : path
        defined =
          begin
            !name.empty? && mod.const_defined?(name, false)
          rescue NameError
            false
          end
        raise ArgumentError, "undefined class/module #{so_far}" unless defined
        mod = mod.const_get(name, false)
        unless Module === mod
          raise TypeError, "#{path} does not refer to class/module"
        end
        break if stop.nil?
        offset = stop + 2
      end
      mod
    end
  end

  class Parser
    # Any encoding: Let the parser choose the encoding
    ANY = 0
    # UTF-8 Encoding
    UTF8 = 1
    # UTF-16-LE Encoding with BOM
    UTF16LE = 2
    # UTF-16-BE Encoding with BOM
    UTF16BE = 3

    # call-seq:
    #    parser.mark # => #<Psych::Parser::Mark>
    #
    # Returns a Psych::Parser::Mark object that contains line, column, and
    # index information.
    def mark
      event = @__mark__
      return Mark.new(0, 0, 0) if event.nil?
      Mark.new(event[5], event[6], event[7])
    end

    private

    def _native_parse(handler, yaml, path)
      @__mark__ = nil
      if yaml.respond_to?(:read)
        encoding = Parser.__io_encoding__(yaml)
        input = "".b
        # As libyaml's reader asks `io_reader`, until a read answers nil or
        # nothing.
        while (chunk = yaml.read(16384))
          chunk = IO.__str__(chunk)
          break if chunk.empty?
          input << chunk.b
        end
      else
        input, encoding = Parser.__transcode__(IO.__str__(yaml))
      end
      events, error = __psych_parse__(input, encoding)
      internal = Encoding.default_internal
      Parser.__internal__(events, internal) unless internal.nil?
      events.each do |event|
        # The event's mark, its fifth to seventh elements, is `mark`'s.
        @__mark__ = event
        handler.event_location(event[1], event[2], event[3], event[4])
        case event[0]
        when 5 then handler.scalar(event[8], event[9], event[10], event[11], event[12], event[13])
        when 8 then handler.start_mapping(event[8], event[9], event[10], event[11])
        when 9 then handler.end_mapping
        when 6 then handler.start_sequence(event[8], event[9], event[10], event[11])
        when 7 then handler.end_sequence
        when 4 then handler.alias(event[8])
        when 2 then handler.start_document(event[8], event[9], event[10])
        when 3 then handler.end_document(event[8])
        when 0 then handler.start_stream(event[8])
        when 1 then handler.end_stream
        else handler.empty
        end
      end
      if error
        # A failed parse leaves the parser re-initialised, as `parse` does.
        @__mark__ = nil
        line, column, offset, problem, context = error
        raise NoMemoryError if line.zero?
        raise Psych::SyntaxError.new(path, line, column, offset, problem, context)
      end
      self
    end

    # PSYCH_TRANSCODE: each String the events hold, which is UTF-8, in the
    # default internal encoding, `internal`.
    def self.__internal__(events, internal)
      export = ->(string) { string.nil? ? nil : Psych.__send__(:__export__, string, internal) }
      events.each do |event|
        case event[0]
        when 2 then event[9] = event[9].map { |handle, prefix| [export.(handle), export.(prefix)] }
        when 4 then event[8] = export.(event[8])
        when 5, 6, 8
          event[8] = export.(event[8])
          event[9] = export.(event[9])
          event[10] = export.(event[10]) if event[0] == 5
        end
      end
    end

    # `transcode_string`: the bytes to parse and the encoding to read them
    # as. UTF-8 and UTF-16 are read as they are; anything else is converted
    # to UTF-8 if it can be, and read as UTF-8 either way.
    def self.__transcode__(yaml)
      case yaml.encoding
      when Encoding::UTF_8 then [yaml, UTF8]
      when Encoding::UTF_16LE then [yaml, UTF16LE]
      when Encoding::UTF_16BE then [yaml, UTF16BE]
      else [Psych.__send__(:__export__, yaml), UTF8]
      end
    end

    # `transcode_io`: the encoding to read an IO's bytes as, from its
    # external encoding.
    def self.__io_encoding__(io)
      encoding = io.external_encoding
      encoding = Encoding.find(encoding) unless encoding.nil? || Encoding === encoding
      case encoding
      when Encoding::US_ASCII, Encoding::UTF_8 then UTF8
      when Encoding::UTF_16LE then UTF16LE
      when Encoding::UTF_16BE then UTF16BE
      else ANY
      end
    end
  end

  class Emitter < Handler
    # call-seq: Psych::Emitter.new(io, options = Psych::Emitter::OPTIONS)
    #
    # Create a new Psych::Emitter that writes to +io+.
    def initialize(io, options = (no_options = true; nil))
      unless no_options
        line_width = options.line_width
        indentation = options.indentation
        canonical = options.canonical
        self.line_width = line_width
        self.indentation = indentation
        self.canonical = canonical
      end
      @__io__ = io
      self
    end

    # A copy has an emitter of its own, as `dup` of the C class allocates
    # one, with the original's IO.
    def initialize_copy(other)
      super
      @__emitter__ = nil
    end

    # call-seq: emitter.start_stream(encoding)
    #
    # Start a stream emission with +encoding+
    #
    # See Psych::Handler#start_stream
    def start_stream(encoding)
      unless Integer === encoding
        raise TypeError, "wrong argument type #{Psych.__send__(:__type_name__, encoding)} (expected Integer)"
      end
      __emitter_emit__([0, IO.__c_int__(encoding)])
      # What libyaml's stream start makes of the settings.
      state = __emitter_state__
      state[2] = 2 if state[2] < 2 || state[2] > 9
      state[1] = 80 if state[1] >= 0 && state[1] <= state[2] * 2
      state[1] = 2147483647 if state[1] < 0
      self
    end

    # call-seq: emitter.end_stream
    #
    # End a stream emission
    #
    # See Psych::Handler#end_stream
    def end_stream
      __emitter_emit__([1])
      self
    end

    # call-seq: emitter.start_document(version, tags, implicit)
    #
    # Start a document emission with YAML +version+, +tags+, and an +implicit+
    # start.
    #
    # See Psych::Handler#start_document
    def start_document(version, tags, imp)
      Psych.__send__(:__check_array__, version)
      version = version.empty? ? [] : [IO.__c_int__(version[0]), IO.__c_int__(version[1])]
      directives = []
      if tags
        Psych.__send__(:__check_array__, tags)
        tags.each do |tuple|
          Psych.__send__(:__check_array__, tuple)
          raise RuntimeError, "tag tuple must be of length 2" if tuple.length < 2
          name = Psych.__send__(:__export__, IO.__str__(tuple[0]))
          value = Psych.__send__(:__export__, IO.__str__(tuple[1]))
          directives << [Psych.__send__(:__cstr__, name), Psych.__send__(:__cstr__, value)]
        end
      end
      __emitter_emit__([2, version, directives, imp ? true : false])
      self
    end

    # call-seq: emitter.end_document(implicit)
    #
    # End a document emission with an +implicit+ ending.
    #
    # See Psych::Handler#end_document
    def end_document(imp)
      __emitter_emit__([3, imp ? true : false])
      self
    end

    # call-seq: emitter.scalar(value, anchor, tag, plain, quoted, style)
    #
    # Emit a scalar with +value+, +anchor+, +tag+, and a +plain+ or +quoted+
    # string type with +style+.
    #
    # See Psych::Handler#scalar
    def scalar(value, anchor, tag, plain, quoted, style)
      Psych.__send__(:__check_string__, value) unless String === value
      value = Psych.__send__(:__export__, value) unless value.encoding.equal?(Encoding::UTF_8)
      anchor = __emitter_name__(anchor) unless anchor.nil?
      tag = __emitter_name__(tag) unless tag.nil?
      style = __emitter_int__(style)
      __emitter_emit__([5, value, anchor, tag, plain ? true : false, quoted ? true : false, style])
      self
    end

    # call-seq: emitter.start_sequence(anchor, tag, implicit, style)
    #
    # Start emitting a sequence with +anchor+, a +tag+, +implicit+ sequence
    # start and end, along with +style+.
    #
    # See Psych::Handler#start_sequence
    def start_sequence(anchor, tag, implicit, style)
      anchor = __emitter_name__(anchor) unless anchor.nil?
      tag = __emitter_name__(tag) unless tag.nil?
      __emitter_emit__([6, anchor, tag, implicit ? true : false, __emitter_int__(style)])
      self
    end

    # call-seq: emitter.end_sequence
    #
    # End sequence emission.
    #
    # See Psych::Handler#end_sequence
    def end_sequence
      __emitter_emit__([7])
      self
    end

    # call-seq: emitter.start_mapping(anchor, tag, implicit, style)
    #
    # Start emitting a YAML map with +anchor+, +tag+, an +implicit+ start
    # and end, and +style+.
    #
    # See Psych::Handler#start_mapping
    def start_mapping(anchor, tag, implicit, style)
      anchor = __emitter_name__(anchor) unless anchor.nil?
      tag = __emitter_name__(tag) unless tag.nil?
      __emitter_emit__([8, anchor, tag, implicit ? true : false, __emitter_int__(style)])
      self
    end

    # call-seq: emitter.end_mapping
    #
    # Emit the end of a mapping.
    #
    # See Psych::Handler#end_mapping
    def end_mapping
      __emitter_emit__([9])
      self
    end

    # call-seq: emitter.alias(anchor)
    #
    # Emit an alias with +anchor+.
    #
    # See Psych::Handler#alias
    def alias(anchor)
      __emitter_emit__([4, __emitter_name__(anchor)])
      self
    end

    # call-seq: emitter.canonical = true
    #
    # Set the output style to canonical, or not.
    def canonical=(style)
      on = true.equal?(style)
      __psych_emitter_set__(__emitter_id__, 2, on)
      __emitter_state__[3] = on
      style
    end

    # call-seq: emitter.canonical
    #
    # Get the output style, canonical or not.
    def canonical
      __emitter_state__[3]
    end

    # call-seq: emitter.indentation = level
    #
    # Set the indentation level to +level+.  The level must be less than 10
    # and greater than 1.
    def indentation=(level)
      level = IO.__c_int__(level)
      __psych_emitter_set__(__emitter_id__, 1, level)
      __emitter_state__[2] = 1 < level && level < 10 ? level : 2
      level
    end

    # call-seq: emitter.indentation
    #
    # Get the indentation level.
    def indentation
      __emitter_state__[2]
    end

    # call-seq: emitter.line_width
    #
    # Get the preferred line width.
    def line_width
      __emitter_state__[1]
    end

    # call-seq: emitter.line_width = width
    #
    # Set the preferred line with to +width+.
    def line_width=(width)
      width = IO.__c_int__(width)
      __psych_emitter_set__(__emitter_id__, 0, width)
      __emitter_state__[1] = width >= 0 ? width : -1
      width
    end

    private

    # This emitter's holder, `[id, line_width, indentation, canonical]`,
    # made the first time it is needed, with `allocate`'s settings: a fresh
    # libyaml emitter's width, an indentation of two.
    def __emitter_state__
      @__emitter__ ||= begin
        id = __psych_emitter_alloc__
        raise NoMemoryError, "failed to allocate memory" if id.nil?
        Psych.__send__(:__holder__, id).push(0, 2, false)
      end
    end

    def __emitter_id__ = __emitter_state__[0]

    # `NUM2INT`: a style, which is almost always a small Integer already.
    def __emitter_int__(value)
      return value if Integer === value && value >= -2147483648 && value <= 2147483647
      IO.__c_int__(value)
    end

    # An anchor or tag: nil, or a String libyaml takes as a C string.
    def __emitter_name__(name)
      return nil if name.nil?
      name = Psych.__send__(:__export__, Psych.__send__(:__check_string__, name))
      Psych.__send__(:__cstr__, name)
    end

    # Emit one event, writing what libyaml wrote to the IO, as
    # `psych_emitter.c`'s `writer` does when libyaml flushes.
    def __emitter_emit__(event)
      holder = @__emitter__
      written = __psych_emit__(holder ? holder[0] : __emitter_id__, event)
      return if written.empty?
      raise RuntimeError, written[0] if Array === written
      wrote = @__io__.write(written)
      raise RuntimeError, "write error" if IO.__c_int__(wrote).zero?
    end
  end
end
