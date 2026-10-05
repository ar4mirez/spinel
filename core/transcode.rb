# `String#encode`, `#encode!` and `#scrub` (#19), driving the transcoder in
# `crates/spinel-vm/src/transcode.rs` one step at a time. The VM converts
# between the encodings whose mapping is arithmetic — UTF-8, US-ASCII,
# BINARY, ISO-8859-1, UTF-16 and UTF-32 — and refuses any other; this file
# is CRuby's rules about what an error means: `invalid:`/`undef:` with
# `replace:`, `fallback:`, `xml:`, the newline options, and the exceptions,
# with their attributes and messages, measured on ruby 4.0.7.
class Encoding
  class InvalidByteSequenceError
    def error_bytes = @__error_bytes__
    def readagain_bytes = @__readagain_bytes__
    def source_encoding = @__source_encoding__
    def destination_encoding = @__destination_encoding__
    def source_encoding_name = @__source_encoding__&.name
    def destination_encoding_name = @__destination_encoding__&.name
    # nil until a conversion sets it, then true or false. Measured.
    def incomplete_input? = @__incomplete__

    def __set__(error, readagain, source, destination, incomplete)
      @__error_bytes__ = error
      @__readagain_bytes__ = readagain
      @__source_encoding__ = source
      @__destination_encoding__ = destination
      @__incomplete__ = incomplete
      self
    end
  end

  class UndefinedConversionError
    def error_char = @__error_char__
    def source_encoding = @__source_encoding__
    def destination_encoding = @__destination_encoding__
    def source_encoding_name = @__source_encoding__&.name
    def destination_encoding_name = @__destination_encoding__&.name

    def __set__(char, source, destination)
      @__error_char__ = char
      @__source_encoding__ = source
      @__destination_encoding__ = destination
      self
    end
  end
end

class String
  UNICODE_INDEXES = [1, 3, 4, 5, 6, 7, 8].freeze

  def encode(*args, **options)
    __transcoded__(args, options)
  end

  def encode!(*args, **options)
    __modify__
    result = __transcoded__(args, options)
    __splice__(0, bytesize, result)
    __force_encoding__(result.__encoding_index__)
  end

  # The target and source as Encodings: a missing target is the default
  # internal encoding, and with none the string keeps its own.
  def __transcoded__(args, options)
    if args.size > 2
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
    end
    source = args.size == 2 ? __converter_encoding__(args[1], encoding) : encoding
    target =
      if args.empty?
        Encoding.default_internal || source
      else
        __converter_encoding__(args[0], source)
      end
    text = __decorate_source__(self, source, options)
    if target.equal?(source) && options[:invalid] != :replace
      xml = options[:xml]
      if xml && !(xml == :text || xml == :attr)
        raise ArgumentError, "unexpected value for xml option: #{xml.inspect}"
      end
      # `xml:` escapes even when nothing is transcoded. Measured.
      result = xml ? String.__escape_xml__(text, xml) : text.dup
      result = __decorate_output__(result, options)
      return result.force_encoding(target)
    end
    String.__convert__(text, source, target, options)
  end

  def __converter_encoding__(value, from)
    return value if value.is_a?(Encoding)
    name = String.__coerce__(value)
    Encoding.find(name)
  rescue ArgumentError
    raise Encoding::ConverterNotFoundError, "code converter not found (#{from.name} to #{name})"
  end

  # The newline options that read the source: CRLF and CR become LF.
  def __decorate_source__(text, source, options)
    return text unless options[:universal_newline] && source.ascii_compatible?
    text.gsub("\r\n", "\n").gsub("\r", "\n")
  end

  def __decorate_output__(text, options)
    if options[:crlf_newline] && text.encoding.ascii_compatible?
      text = text.gsub("\n", "\r\n")
    elsif options[:cr_newline] && text.encoding.ascii_compatible?
      text = text.gsub("\n", "\r")
    end
    text
  end

  def self.__unicode__(encoding)
    UNICODE_INDEXES.include?(encoding.__index__)
  end

  # The step loop. A BOM'd UTF-16 or UTF-32 target converts big-endian and
  # gains its BOM; a source of either reads its BOM to learn its order.
  def self.__convert__(text, source, target, options)
    xml = options[:xml]
    if xml && !(xml == :text || xml == :attr)
      raise ArgumentError, "unexpected value for xml option: #{xml.inspect}"
    end
    replacement = __replacement__(target, options)
    bom = nil
    step_target = target
    if target.__index__ == 7 || target.__index__ == 8
      bom = target.__index__ == 7 ? "\xFE\xFF".b : "\x00\x00\xFE\xFF".b
      step_target = Encoding::LIST[target.__index__ == 7 ? 3 : 5]
    end
    step_source = source
    start = 0
    if source.__index__ == 7 || source.__index__ == 8
      step_source, start = __bom_order__(text, source)
    end
    text = __escape_xml__(text, xml) if xml
    out = String.new(encoding: Encoding::BINARY)
    out << bom if bom
    loop do
      step = text.__transcode__(step_source.__index__, step_target.__index__, start)
      __needs_char_table__ if step.nil?
      output, stop, error, readagain, following, code = step
      out << output.b
      break if stop == :done
      piece =
        case stop
        when :invalid, :incomplete
          unless options[:invalid] == :replace
            raise __invalid_error__(stop, error, readagain, source, target)
          end
          replacement
        when :undefined
          char = code ? Integer.__utf8__(code) : error.dup.force_encoding(source)
          char = char.encode(source) if code && !source.equal?(Encoding::UTF_8) && __supported_target__(source)
          if xml && code
            "&#x#{Integer.__hex__(code, 1)};"
          elsif options[:undef] == :replace
            replacement
          elsif options[:fallback]
            __fallback__(options[:fallback], char, code, error, source, target)
          else
            raise __undefined_error__(char, code, error, source, target)
          end
        end
      out << __in_target__(piece, step_target).b
      start = following
    end
    out = __decorate_output__(out.force_encoding(target), options) if options[:crlf_newline] || options[:cr_newline]
    out.force_encoding(target)
  end

  def self.__supported_target__(encoding)
    [0, 1, 2, 3, 4, 5, 6, 22].include?(encoding.__index__)
  end

  # What `invalid: :replace` and `undef: :replace` substitute: `replace:`, or
  # U+FFFD for a Unicode target and "?" for any other.
  def self.__replacement__(target, options)
    replacement = options[:replace]
    return String.__coerce__(replacement) unless replacement.nil?
    __unicode__(target) ? "�" : "?"
  end

  # A piece of output in the target encoding; a replacement that cannot be
  # converted is CRuby's ConverterNotFoundError, oddly but measured.
  def self.__in_target__(piece, target)
    return piece if piece.encoding.equal?(target) || piece.empty?
    piece.encode(target)
  rescue Encoding::UndefinedConversionError
    raise Encoding::ConverterNotFoundError, "code converter not found (#{piece.encoding.name} to #{target.name})"
  end

  def self.__fallback__(fallback, char, code, error, source, target)
    result =
      if fallback.is_a?(Hash)
        fallback[char]
      elsif fallback.respond_to?(:call)
        fallback.call(char)
      else
        fallback[char]
      end
    raise __undefined_error__(char, code, error, source, target) if result.nil?
    result = String.__coerce__(result)
    begin
      result.encode(target)
    rescue Encoding::UndefinedConversionError
      raise ArgumentError, "too big fallback string"
    end
  end

  def self.__bom_order__(text, source)
    bytes = text.__bytes__
    if source.__index__ == 7
      return [Encoding::UTF_16LE, 2] if bytes[0] == 0xff && bytes[1] == 0xfe
      return [Encoding::UTF_16BE, 2] if bytes[0] == 0xfe && bytes[1] == 0xff
      return [Encoding::UTF_16BE, 0]
    end
    return [Encoding::UTF_32LE, 4] if bytes[0, 4] == [0xff, 0xfe, 0, 0]
    return [Encoding::UTF_32BE, 4] if bytes[0, 4] == [0, 0, 0xfe, 0xff]
    [Encoding::UTF_32BE, 0]
  end

  def self.__escape_xml__(text, mode)
    escaped = String.new(encoding: text.encoding)
    escaped << "\"" if mode == :attr
    text.each_char do |c|
      escaped <<
        case c
        when "&" then "&amp;"
        when "<" then "&lt;"
        when ">" then "&gt;"
        when "\"" then mode == :attr ? "&quot;" : c
        else c
        end
    end
    escaped << "\"" if mode == :attr
    escaped
  end

  def self.__invalid_error__(stop, error, readagain, source, target)
    shown = error.inspect
    message =
      if stop == :incomplete
        "incomplete #{shown} on #{source.name}"
      elsif readagain.empty?
        "#{shown} on #{source.name}"
      else
        "#{shown} followed by #{readagain.inspect} on #{source.name}"
      end
    # No bytes to read again is nil, not "". Measured.
    readagain = nil if readagain.empty?
    Encoding::InvalidByteSequenceError.new(message).__set__(error, readagain, source, target, stop == :incomplete)
  end

  # A codepoint names itself as `U+00E9`; a BINARY byte as its escape. A
  # conversion that goes through UTF-8 says so. Measured.
  def self.__undefined_error__(char, code, error, source, target)
    message =
      if code.nil?
        "#{error.inspect} from #{source.name} to #{target.name}"
      elsif !source.equal?(Encoding::UTF_8) && !target.equal?(Encoding::UTF_8)
        "U+#{Integer.__hex__(code, 4)} to #{target.name} in conversion from #{source.name} to UTF-8 to #{target.name}"
      else
        "U+#{Integer.__hex__(code, 4)} from #{source.name} to #{target.name}"
      end
    # Through UTF-8, the step that failed is UTF-8's: its character and its
    # source encoding are what the error reports.
    if code && !source.equal?(Encoding::UTF_8) && !target.equal?(Encoding::UTF_8)
      return Encoding::UndefinedConversionError.new(message).__set__(Integer.__utf8__(code), Encoding::UTF_8, target)
    end
    Encoding::UndefinedConversionError.new(message).__set__(char, source, target)
  end

  # `scrub`: each invalid sequence replaced — by the argument, by the block's
  # answer for its bytes, or by U+FFFD or "?". A valid string comes back as a
  # copy.
  def scrub(replacement = nil, &block)
    unless replacement.nil?
      replacement = String.__coerce__(replacement)
      unless replacement.valid_encoding?
        raise ArgumentError, "replacement must be valid byte sequence '#{replacement.inspect}'"
      end
      __combined_encoding__(replacement)
    end
    # Always a String, never the receiver's subclass. Measured.
    return String.new(self) if valid_encoding?
    default = String.__unicode__(encoding) ? "�" : "?"
    out = String.new(encoding: encoding)
    start = 0
    loop do
      step = __transcode__(__encoding_index__, __encoding_index__, start)
      __needs_char_table__ if step.nil?
      output, stop, error, _readagain, following = step
      out << output
      break if stop == :done
      piece = block ? String.__coerce__(block.call(error.dup)) : (replacement || default)
      out << piece
      start = following
    end
    out
  end

  def scrub!(replacement = nil, &block)
    __modify__
    result = scrub(replacement, &block)
    __splice__(0, bytesize, result) unless result == self
    self
  end
end

# `Encoding::Converter` (#19): the streaming interface to the same step
# primitive `encode` uses. It keeps what CRuby's keeps between calls — bytes
# read past an error to be read again, input that ended mid-character, output
# that did not fit, and the last error — so `primitive_convert` can be called
# repeatedly over a stream. Pairs the VM cannot convert are refused at `new`.
class Encoding
  class Converter
    INVALID_MASK = 0xf
    INVALID_REPLACE = 0x2
    UNDEF_MASK = 0xf0
    UNDEF_REPLACE = 0x20
    UNDEF_HEX_CHARREF = 0x30
    UNIVERSAL_NEWLINE_DECORATOR = 0x100
    CRLF_NEWLINE_DECORATOR = 0x1000
    CR_NEWLINE_DECORATOR = 0x2000
    LF_NEWLINE_DECORATOR = 0x4000
    XML_TEXT_DECORATOR = 0x8000
    XML_ATTR_CONTENT_DECORATOR = 0x10000
    PARTIAL_INPUT = 0x20000
    AFTER_OUTPUT = 0x40000
    XML_ATTR_QUOTE_DECORATOR = 0x100000

    attr_reader :source_encoding, :destination_encoding, :last_error

    def self.__encoding__(value)
      return value if value.is_a?(Encoding)
      Encoding.find(String.__coerce__(value))
    end

    # The converters run through UTF-8 unless one side is UTF-8.
    def self.__path__(source, destination)
      if source.equal?(Encoding::UTF_8) || destination.equal?(Encoding::UTF_8)
        [[source, destination]]
      else
        [[source, Encoding::UTF_8], [Encoding::UTF_8, destination]]
      end
    end

    def self.search_convpath(source, destination, **options)
      source = __encoding__(source)
      destination = __encoding__(destination)
      # Which pairs CRuby has a path for is its converter table's question.
      unless String.__supported_target__(source) && String.__supported_target__(destination)
        __needs_char_table__
      end
      path = __path__(source, destination)
      path.push("universal_newline") if options[:universal_newline]
      path.push("crlf_newline") if options[:crlf_newline]
      path
    end

    # The ASCII-compatible encoding a non-ASCII-compatible one converts
    # through: UTF-8 for UTF-16 and UTF-32. nil for one that is already
    # ASCII-compatible. Measured.
    def self.asciicompat_encoding(encoding)
      encoding = __encoding__(encoding)
      return nil if encoding.ascii_compatible?
      return Encoding.find("stateless-ISO-2022-JP") if encoding.name == "ISO-2022-JP"
      return Encoding::UTF_8 if [3, 4, 5, 6, 7, 8].include?(encoding.__index__)
      nil
    rescue ArgumentError
      nil
    end

    def initialize(source, destination, options = 0)
      @source_encoding = Converter.__encoding__(source)
      @destination_encoding = Converter.__encoding__(destination)
      if @source_encoding.equal?(@destination_encoding)
        raise Encoding::ConverterNotFoundError,
              "code converter not found (#{@source_encoding.name} to #{@destination_encoding.name})"
      end
      unless String.__supported_target__(@source_encoding) && String.__supported_target__(@destination_encoding)
        __needs_char_table__
      end
      @flags = 0
      @decorators = []
      replacement = nil
      if options.is_a?(Integer)
        @flags = options
      else
        options = options.to_hash unless options.is_a?(Hash)
        @flags = @flags | INVALID_REPLACE if options[:invalid] == :replace
        @flags = @flags | UNDEF_REPLACE if options[:undef] == :replace
        @flags = @flags | UNDEF_HEX_CHARREF if options[:xml]
        @flags = @flags | UNIVERSAL_NEWLINE_DECORATOR if options[:universal_newline]
        @flags = @flags | CRLF_NEWLINE_DECORATOR if options[:crlf_newline]
        @flags = @flags | CR_NEWLINE_DECORATOR if options[:cr_newline]
        replacement = options[:replace]
      end
      @decorators.push("universal_newline") if @flags & UNIVERSAL_NEWLINE_DECORATOR != 0
      @decorators.push("crlf_newline") if @flags & CRLF_NEWLINE_DECORATOR != 0
      @decorators.push("cr_newline") if @flags & CR_NEWLINE_DECORATOR != 0
      self.replacement = replacement.nil? ? __default_replacement__ : replacement
      @readagain = "".b
      @pending_output = "".b
      @errinfo = [:source_buffer_empty, nil, nil, nil, nil]
      @last_error = nil
    end

    def __default_replacement__
      String.__unicode__(@destination_encoding) ? "�" : "?".encode(Encoding::US_ASCII)
    end

    def replacement
      @replacement
    end

    def replacement=(value)
      unless value.is_a?(String)
        unless value.respond_to?(:to_str) && !(value == true || value == false || value.is_a?(Integer))
          raise TypeError, "no implicit conversion of #{value.inspect} into String" if value == true || value == false
          raise TypeError, "no implicit conversion of #{value.class} into String"
        end
        converted = value.to_str
        raise TypeError, "can't convert #{value.class} to String (#{value.class}#to_str gives #{converted.class})" unless converted.is_a?(String)
        value = converted
      end
      @replacement = value
    end

    def convpath
      Converter.__path__(@source_encoding, @destination_encoding) + @decorators
    end

    def inspect
      "#<Encoding::Converter: #{@source_encoding.name} to #{@destination_encoding.name}>"
    end

    def primitive_errinfo
      @errinfo.dup
    end

    def putback(max = nil)
      taken = max.nil? ? @readagain : @readagain.byteslice(@readagain.bytesize - [max, @readagain.bytesize].min, max)
      @readagain = @readagain.byteslice(0, @readagain.bytesize - taken.bytesize)
      taken.force_encoding(@source_encoding)
    end

    def insert_output(text)
      @pending_output << String.__in_target__(String.__coerce__(text), @destination_encoding).b
      nil
    end

    # Converts as much of `source` as it can into `destination` and says why
    # it stopped. Consumed bytes leave `source`; bytes read past an error are
    # kept for the next call; output past `bytesize` waits for the next call.
    def primitive_convert(source, destination, offset = nil, bytesize = nil, options = nil)
      Kernel.__check_frozen__(destination)
      flags = @flags
      if options.is_a?(Integer)
        flags = flags | options
      elsif options.is_a?(Hash)
        flags = flags | PARTIAL_INPUT if options[:partial_input]
        flags = flags | AFTER_OUTPUT if options[:after_output]
      end
      offset = Integer.__index__(offset) unless offset.nil?
      bytesize = Integer.__index__(bytesize) unless bytesize.nil?
      raise ArgumentError, "output_byteoffset too big" if !offset.nil? && offset > destination.bytesize
      input = @readagain + (source.nil? ? "".b : source.b)
      @readagain = "".b
      source&.__splice__(0, source.bytesize, "")
      output = @pending_output
      @pending_output = "".b
      result = __run__(input, output, flags, source)
      unless offset.nil?
        destination.__splice__(offset, destination.bytesize - offset, "")
      end
      if !bytesize.nil? && output.bytesize > bytesize
        @pending_output = output.byteslice(bytesize, output.bytesize - bytesize)
        output = output.byteslice(0, bytesize)
        result = :destination_buffer_full
      end
      destination.__splice__(destination.bytesize, 0, output)
      destination.force_encoding(@destination_encoding)
      @errinfo[0] = result
      result
    end

    # One run over `input`, appending to `output`. Errors the flags replace
    # are replaced; the first one they do not is reported, its bytes and any
    # read-again bytes taken from the input, and what follows put back in
    # `source` for the caller.
    def __run__(input, output, flags, source)
      start = 0
      loop do
        step = input.__transcode__(@source_encoding.__index__, @destination_encoding.__index__, start)
        converted, stop, error, readagain, following, code = step
        output << converted.b
        if stop == :done
          @errinfo = [flags & PARTIAL_INPUT != 0 ? :source_buffer_empty : :finished, nil, nil, nil, nil]
          @last_error = nil
          return @errinfo[0]
        end
        if stop == :incomplete && flags & PARTIAL_INPUT != 0
          @readagain = error.b
          @errinfo = [:source_buffer_empty, nil, nil, nil, nil]
          return :source_buffer_empty
        end
        replace =
          if stop == :undefined
            if flags & UNDEF_MASK == UNDEF_HEX_CHARREF && code
              "&#x#{Integer.__hex__(code, 1)};"
            elsif flags & UNDEF_MASK == UNDEF_REPLACE
              @replacement
            end
          elsif flags & INVALID_MASK == INVALID_REPLACE
            @replacement
          end
        if replace
          output << String.__in_target__(replace, @destination_encoding).b
          start = following
          next
        end
        rest = input.byteslice(following + readagain.bytesize, input.bytesize)
        @readagain = readagain.b
        source&.__splice__(0, 0, rest)
        result = { invalid: :invalid_byte_sequence, incomplete: :incomplete_input, undefined: :undefined_conversion }[stop]
        if stop == :undefined
          char = code ? Integer.__utf8__(code) : error.dup.force_encoding(@source_encoding)
          @last_error = String.__undefined_error__(char, code, error, @source_encoding, @destination_encoding)
          @errinfo = [result, @source_encoding.name, @destination_encoding.name, error.b, "".b]
        else
          @last_error = String.__invalid_error__(stop, error.b, readagain.b, @source_encoding, @destination_encoding)
          @errinfo = [result, @source_encoding.name, @destination_encoding.name, error.b, readagain.b]
        end
        return result
      end
    end

    # Converts a chunk, raising the error a stop names; input that ends
    # mid-character waits for more, or for `finish`.
    def convert(text)
      text = String.__coerce__(text)
      source = text.dup
      out = String.new(encoding: @destination_encoding)
      loop do
        result = primitive_convert(source, out, nil, nil, partial_input: true)
        case result
        when :source_buffer_empty, :finished then return out
        when :destination_buffer_full then next
        else raise @last_error
        end
      end
    end

    def finish
      out = String.new(encoding: @destination_encoding)
      result = primitive_convert("".b, out)
      raise @last_error if result == :invalid_byte_sequence || result == :incomplete_input || result == :undefined_conversion
      out
    end
  end
end
