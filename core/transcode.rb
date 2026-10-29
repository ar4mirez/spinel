# `String#encode`, `#encode!` and `#scrub` (#19), driving the transcoder in
# `crates/spinel-vm/src/transcode.rs` one step at a time. The VM converts
# between the encodings whose mapping is arithmetic — UTF-8, US-ASCII,
# BINARY, ISO-8859-1, UTF-16 and UTF-32 — EUC-JP, Shift_JIS and
# Windows-31J, whose mapping is CRuby's table (#378), and ISO-2022-JP and
# stateless-ISO-2022-JP (#404), and refuses any other. This file finds the
# path of CRuby's converters a conversion takes, and is CRuby's rules about
# what an error means: `invalid:`/`undef:` with `replace:`, `fallback:`,
# `xml:`, the newline options, and the exceptions, with their attributes and
# messages, measured on ruby 4.0.7 and 4.0.9.
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
  # `newline:`'s values, and the decorators they name.
  NEWLINE_DECORATORS = {
    universal: "universal_newline", crlf: "crlf_newline", cr: "cr_newline", lf: "lf_newline"
  }.freeze

  # How the VM names a newline decorator: see `transcode::step`. CRuby's
  # `lf_newline` is `universal_newline` again. Measured.
  NEWLINE_MODES = { "universal_newline" => 1, "lf_newline" => 1, "crlf_newline" => 2, "cr_newline" => 3 }.freeze

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
    text = self
    xml = options[:xml]
    if xml && !(xml == :text || xml == :attr)
      raise ArgumentError, "unexpected value for xml option: #{xml}" if xml.is_a?(Symbol)
      raise ArgumentError, "unexpected value for xml option"
    end
    newlines = String.__newline_decorators__(options)
    decorated = xml || !newlines.empty?
    # CRuby's `str_transcode0` again: with no decorator, ASCII between two
    # ASCII-compatible encodings is relabelled, not converted, so no
    # converter is needed and none refuses a character. Measured.
    if !decorated && !target.equal?(source) && source.ascii_compatible? && target.ascii_compatible? && ascii_only?
      return dup.force_encoding(target)
    end
    if decorated && !source.ascii_compatible? && !target.ascii_compatible?
      # CRuby's `str_transcode0`: with a decorator between two encodings that
      # are not ASCII-compatible, the string goes to UTF-8 first — and if it
      # cannot, its bytes are read as UTF-8 as they are. Measured.
      text =
        begin
          String.__convert__(self, source, Encoding::UTF_8, {}, nil)
        rescue EncodingError
          b.force_encoding(Encoding::UTF_8)
        end
      source = Encoding::UTF_8
    end
    # One newline decorator at a time: CRuby has no converter for two.
    # Measured.
    raise String.__decorators_not_found__(source, target, newlines, xml) if newlines.size > 1
    # A dummy has no characters to replace, and with a decorator nothing is
    # converted to find one: it is kept as it is, and the decorators work on
    # its bytes. `xml:` escapes even when nothing is transcoded. Measured.
    if target.equal?(source) && (decorated || options[:invalid] != :replace || source.dummy?)
      result = xml ? String.__escape_xml__(text, xml) : text.dup
      return String.__newlines__(result, newlines.first).force_encoding(target)
    end
    String.__convert__(text, source, target, options, newlines.first)
  end

  def __converter_encoding__(value, from)
    return value if value.is_a?(Encoding)
    name = String.__coerce__(value)
    Encoding.find(name)
  rescue ArgumentError
    raise Encoding::ConverterNotFoundError, "code converter not found (#{from.name} to #{name})"
  end

  # The newline decorators `options` ask for, by CRuby's names: `newline:`'s,
  # which overrides the others, or each of `universal_newline:`,
  # `crlf_newline:`, `cr_newline:` and `lf_newline:` that is truthy, in that
  # order. Measured.
  def self.__newline_decorators__(options)
    newline = options[:newline]
    unless newline.nil?
      name = NEWLINE_DECORATORS[newline] if newline.is_a?(Symbol)
      return [name] if name
      raise ArgumentError, "unexpected value for newline option: #{newline}" if newline.is_a?(Symbol)
      raise ArgumentError, "unexpected value for newline option"
    end
    names = []
    names << "universal_newline" if options[:universal_newline]
    names << "crlf_newline" if options[:crlf_newline]
    names << "cr_newline" if options[:cr_newline]
    names << "lf_newline" if options[:lf_newline]
    names
  end

  # The error for decorators no converter path has: the encodings, unless
  # they are the same, and every decorator, the newline ones first. Measured.
  def self.__decorators_not_found__(source, target, newlines, xml)
    names = newlines + (xml == :attr ? ["xml_attr_content", "xml_attr_quote"] : xml ? ["xml_text"] : [])
    names = names.join(",")
    names = "#{source.name} to #{target.name} with #{names}" unless source.equal?(target)
    Encoding::ConverterNotFoundError.new("code converter not found (#{names})")
  end

  # CRLF and CR become LF, byte by byte: an invalid byte is left for the
  # transcoder to report, not refused as a String operation would (#412).
  # No ASCII-compatible encoding has CR or LF as a trailing byte.
  def self.__universal_newline__(text)
    text.b.gsub("\r\n", "\n").gsub("\r", "\n").force_encoding(text.encoding)
  end

  # The newline decorator `name` on the bytes of `text`, which are ASCII
  # where they are ASCII: before a source is read, or with nothing to
  # transcode.
  def self.__newlines__(text, name)
    case name
    when "universal_newline", "lf_newline" then __universal_newline__(text)
    when "crlf_newline" then text.b.gsub("\n", "\r\n").force_encoding(text.encoding)
    when "cr_newline" then text.b.gsub("\n", "\r").force_encoding(text.encoding)
    else text
    end
  end

  # What the first decorator hands on at each of its turns, where the
  # decorators read the source itself (UTF-8 into UTF-16 or UTF-32), and
  # whether `universal_newline` holds a CR after them. CRuby's first
  # decorator writes one source byte's worth and stops, and the encoder reads
  # all of it before the next turn. A CR `universal_newline` holds writes
  # nothing: as the first decorator it goes on to the next byte, but behind
  # `xml:` that turn hands on nothing. A `final` last turn is what finishing
  # writes: the attribute's closing quote, a held CR's LF. Measured.
  def self.__decorator_turns__(text, xml, newline, held, final)
    quoted = false
    turns = text.b.bytes.map do |byte|
      piece = byte.chr
      next piece unless xml
      piece = byte == 34 && xml == :attr ? "&quot;".b : __escape_xml__(piece, :text).b
      next piece if xml == :text || quoted
      quoted = true
      "\"".b + piece
    end
    turns << (xml == :attr ? (quoted ? "\"".b : "\"\"".b) : "".b) if final
    decorated = []
    turns.each_with_index do |piece, at|
      piece, held = __newline_turn__(piece, newline, held)
      if final && at == turns.size - 1 && held
        piece << "\n"
        held = false
      end
      # The first decorator goes on past a turn that writes nothing.
      next if piece.empty? && !xml && at < turns.size - 1
      decorated << piece
    end
    [decorated, held]
  end

  # One turn's bytes through the newline decorator `name`, and whether
  # `universal_newline` holds a CR after them.
  def self.__newline_turn__(piece, name, held)
    case name
    when "universal_newline", "lf_newline"
      out = "".b
      piece.each_byte do |byte|
        if held
          out << "\n"
          held = false
          next if byte == 10
        end
        if byte == 13
          held = true
        else
          out << byte.chr
        end
      end
      [out, held]
    else [__newlines__(piece, name).b, held]
    end
  end

  # The decorators' `turns` read by the encoder along `path` as CRuby's reads
  # its decorator's buffer: all that is buffered at each turn, keeping a
  # character it has only begun when more may come. What the block gives
  # for an error — a replacement, its bytes in `via` — goes to the end of
  # what is buffered, after the rest of the turn it stopped in, with the
  # bytes the error reads again after it, and the decorator takes its next
  # turn before the encoder reads on. Measured. A nil from the block stops,
  # handing back the step and what was not read. The output, the state, what
  # is still buffered, and the step that stopped, if one did.
  def self.__read_turns__(turns, buffered, path, state, final, out, via, named, &block)
    size = out.bytesize
    read = __read_turns_in__(turns, buffered.dup, path, state, final, out, via, named, false, &block)
    return read if read
    # ponytail: CRuby never ends where a replacement it buffers breaks a
    # character the turn has only begun (U+FFFD after "\xC3"), each error
    # buffering more than it reads; past a bound the turns are read again
    # from the start with such a replacement before that character, as
    # without decorators. The upgrade is whatever CRuby settles on.
    out.replace(out.byteslice(0, size))
    __read_turns_in__(turns, buffered, path, state, final, out, via, named, true, &block)
  end

  # `__read_turns__` once, a replacement that would follow a character the
  # turn has only begun going before it when `begun`; nil where, without
  # that, the errors buffer more than they read.
  def self.__read_turns_in__(turns, buffered, path, state, final, out, via, named, begun)
    index = 0
    fed = buffered.bytesize
    inserted = 0
    loop do
      if index < turns.size
        buffered << turns[index]
        fed += turns[index].bytesize
        index += 1
      end
      last = final && index == turns.size
      again = false
      start = 0
      loop do
        step = buffered.__transcode__(path, start, state, 0, last)
        __needs_char_table__ if step.nil?
        output, stop, error, readagain, following = step
        state = step[9]
        out << output.b
        if stop == :done
          buffered = "".b
          break
        end
        if stop == :incomplete && !last
          buffered = error.b
          break
        end
        piece = yield step
        rest = buffered.byteslice(following + readagain.bytesize, buffered.bytesize)
        if piece.nil?
          return [out, state, rest + turns.drop(index).join.b, step]
        end
        inserted += 1
        return nil if !begun && inserted > 2 * fed + 64
        bytes = __in_encoding__(piece, via)
        __not_inserted__(piece, named) if bytes.nil?
        # Nothing to insert leaves the bytes read again in the encoder, which
        # reads them first. Measured.
        buffered = if bytes.empty?
          readagain.b + rest
        elsif begun && bytes.getbyte(0) >= 0x80 && __begun__(rest, path)
          bytes + readagain.b + rest
        else
          rest + bytes + readagain.b
        end
        again = true
        break
      end
      return [out, state, buffered, nil] if index == turns.size && !again
    end
  end

  # Whether `bytes` ends in a character begun but not finished, read along
  # `path`.
  def self.__begun__(bytes, path)
    [3, 2, 1].any? do |size|
      next false if bytes.bytesize < size
      tail = bytes.byteslice(bytes.bytesize - size, size)
      tail.getbyte(0) >= 0xC0 && tail.__transcode__(path, 0, 0, 0, false)[1] == :incomplete
    end
  end

  # The bytes of `piece` in `encoding`, or nil when it has no spelling there.
  def self.__in_encoding__(piece, encoding)
    return piece.b if piece.encoding.equal?(encoding) || piece.empty?
    step = piece.__transcode__(__path__(piece.encoding, encoding).map(&:__index__), 0, 0, 0, true)
    __needs_char_table__ if step.nil?
    step[1] == :done ? step[0].b : nil
  end

  def self.__unicode__(encoding)
    UNICODE_INDEXES.include?(encoding.__index__)
  end

  # The step loop. A BOM'd UTF-16 or UTF-32 target converts big-endian and
  # gains its BOM; a source of either reads its BOM to learn its order. The
  # VM runs the decorators — `xml:` and the newline options — where CRuby's
  # work, after a stateful source is decoded and before a stateful target or
  # one that is not ASCII-compatible is encoded, and keeps the conversion's
  # state between steps — ISO-2022-JP's G0, the attribute's quote, a CR
  # `universal_newline` holds — so what is inserted for an error goes
  # through it too.
  def self.__convert__(text, source, target, options, newline)
    xml = options[:xml]
    replacement = __replacement__(target, options)
    bom = nil
    step_target = target
    if target.__index__ == 7 || target.__index__ == 8
      bom = target.__index__ == 7 ? "\xFE\xFF".b : "\x00\x00\xFE\xFF".b
      step_target = Encoding::LIST[target.__index__ == 7 ? 3 : 5]
    end
    state = 0
    out = String.new(encoding: Encoding::BINARY)
    out << bom if bom
    step_source = source
    start = 0
    unread = 0
    if source.__index__ == 7 || source.__index__ == 8
      step_source, start, unread = __bom_order__(text, source, target, options)
    end
    stages = __path__(step_source, step_target)
    path = stages.map(&:__index__)
    mode = (xml == :attr ? 2 : (xml ? 1 : 0)) | NEWLINE_MODES.fetch(newline, 0) << 2
    # What goes in for an error, and whether it is a fallback's answer.
    piece_for = lambda do |step|
      _, stop, error, readagain, _, code, reference, from, to = step
      from = __step_encoding__(from, step_source, source)
      to = __step_encoding__(to, step_target, target)
      case stop
      when :invalid, :incomplete
        raise __invalid_error__(stop, error, readagain, source, target, to) unless options[:invalid] == :replace
        [replacement, false]
      when :undefined
        if xml && reference
          ["&#x#{Integer.__hex__(reference, 1)};", false]
        # With `xml:` an undefined character is a reference or nothing:
        # `undef: :replace` does not apply. Measured.
        elsif options[:undef] == :replace && !xml
          [replacement, false]
        elsif options[:fallback]
          char = __error_char__(code, error, from)
          [__fallback__(options[:fallback], char, code, error, from, to, source, target), true]
        else
          raise __undefined_error__(code, error, from, to, source, target)
        end
      end
    end
    if (xml || newline) && stages.drop(1).none?(&:ascii_compatible?)
      # The decorators read the source itself — UTF-8 into UTF-16 or UTF-32
      # — and work on its bytes before they are read, so an error reports
      # the bytes they wrote, and what goes in for it is read after what
      # they had written. Measured. Only an error's place depends on the
      # turns, so a conversion without one decorates the text whole.
      whole = __newlines__(xml ? __escape_xml__(text, xml) : text, newline).__transcode__(path, 0, state, 0, true)
      __needs_char_table__ if whole.nil?
      if whole[1] == :done
        out << whole[0].b
        out << "".__transcode__(path, 0, whole[9], 0, true)[0].b
        return out.force_encoding(target)
      end
      turns, = __decorator_turns__(text, xml, newline, false, true)
      out, state = __read_turns__(turns, "".b, path, state, true, out, step_source, target) { |step| piece_for.call(step)[0] }
      out << "".__transcode__(path, 0, state, 0, true)[0].b
      return out.force_encoding(target)
    end
    if unread > 0
      opened = "".__transcode__(path, 0, state, mode, false)
      __needs_char_table__ if opened.nil?
      out << opened[0].b
      state = opened[9]
      unread.times do
        out, state = __insert__(out, replacement, step_target, state) || __not_inserted__(replacement, target)
      end
    end
    loop do
      step = text.__transcode__(path, start, state, mode, false)
      __needs_char_table__ if step.nil?
      output, stop, _, _, following = step
      state = step[9]
      out << output.b
      break if stop == :done
      piece, fallback = piece_for.call(step)
      inserted = __insert__(out, piece, step_target, state)
      raise ArgumentError, "too big fallback string" if inserted.nil? && fallback
      out, state = inserted || __not_inserted__(piece, target)
      start = following
    end
    out << "".__transcode__(path, 0, state, mode, true)[0].b
    out.force_encoding(target)
  end

  # A converter's encoding as the conversion names it: a BOM'd UTF-16's or
  # UTF-32's ends are the byte order they convert in.
  def self.__step_encoding__(index, step_encoding, encoding)
    index == step_encoding.__index__ ? encoding : Encoding::LIST[index]
  end

  # `out` with `piece` appended in `target`, and the conversion's state after
  # it. CRuby converts a piece on its own to the target's insert encoding —
  # stateless-ISO-2022-JP for ISO-2022-JP — and then through the target's
  # encoder, so it switches ISO-2022-JP's G0 as the text around it does and
  # leaves the source's decoder as it was. nil when `piece` has no spelling
  # in `target`.
  def self.__insert__(out, piece, target, state)
    return [out, state] if piece.empty?
    via = target.__index__ == 85 ? Encoding::LIST[79] : target
    bytes = piece.b
    unless piece.encoding.equal?(via)
      path = __path__(piece.encoding, via).map(&:__index__)
      step = piece.__transcode__(path, 0, 0, 0, false)
      __needs_char_table__ if step.nil?
      return nil unless step[1] == :done
      bytes = step[0].b
    end
    unless via.equal?(target)
      step = bytes.__transcode__([79, 85], 0, state, 0, false)
      return nil unless step[1] == :done
      bytes = step[0].b
      state = step[9]
    end
    [out << bytes, state]
  end

  # A replacement that cannot be converted is CRuby's ConverterNotFoundError,
  # oddly but measured.
  def self.__not_inserted__(piece, target)
    raise Encoding::ConverterNotFoundError, "code converter not found (#{piece.encoding.name} to #{target.name})"
  end

  def self.__supported_target__(encoding)
    [0, 1, 2, 3, 4, 5, 6, 10, 11, 22, 39, 79, 85].include?(encoding.__index__)
  end

  # Every encoding a conversion passes through, `source` to `target`, along
  # CRuby's converters as `search_convpath` finds them: every encoding to and
  # from UTF-8, EUC-JP to and from Shift_JIS, and ISO-2022-JP through
  # stateless-ISO-2022-JP to and from EUC-JP. Between these the shortest
  # path is the only one. A conversion to the same encoding is the encoding
  # alone. Measured.
  def self.__path__(source, target)
    return [source] if source.equal?(target)
    pair = [source.__index__, target.__index__]
    return [source, target] if pair == [85, 79] || pair == [79, 85]
    if pair.include?(79) || pair.include?(85)
      # Up to EUC-JP and down from it, which is there once.
      return [source, *__to_euc_jp__(source), *[*__to_euc_jp__(target).reverse, target].drop(1)]
    end
    return [source, target] if pair == [10, 39] || pair == [39, 10]
    return [source, target] if source.equal?(Encoding::UTF_8) || target.equal?(Encoding::UTF_8)
    [source, Encoding::UTF_8, target]
  end

  # The encodings from `encoding` to EUC-JP, without `encoding`.
  def self.__to_euc_jp__(encoding)
    euc_jp = Encoding::LIST[10]
    case encoding.__index__
    when 10 then []
    when 85 then [Encoding::LIST[79], euc_jp]
    when 79, 39, 1 then [euc_jp]
    else [Encoding::UTF_8, euc_jp]
    end
  end

  # What `invalid: :replace` and `undef: :replace` substitute: `replace:`, or
  # U+FFFD for a Unicode target and "?" for any other.
  def self.__replacement__(target, options)
    replacement = options[:replace]
    unless replacement.nil?
      replacement = String.__coerce__(replacement)
      raise ArgumentError, "replacement string is broken: #{replacement.dump} as #{replacement.encoding}" unless replacement.valid_encoding?
      return replacement
    end
    __unicode__(target) ? "�" : "?"
  end

  # What `fallback:` answers for an undefined character, as a String.
  def self.__fallback__(fallback, char, code, error, from, to, source, target)
    result =
      if fallback.is_a?(Hash)
        fallback[char]
      elsif fallback.respond_to?(:call)
        fallback.call(char)
      else
        fallback[char]
      end
    raise __undefined_error__(code, error, from, to, source, target) if result.nil?
    String.__coerce__(result)
  end

  # A BOM'd UTF-16 or UTF-32 source read up to its first BOM, which tells
  # its byte order: CRuby's decoder reads every unit before it as invalid
  # and a short one at the end as incomplete, and the BOM is not output.
  # Without a BOM the whole source is read so, and is read in big-endian
  # after it, which then has nothing left to read. The order, where it
  # starts, and how many units before it are replaced. Measured.
  def self.__bom_order__(text, source, target, options)
    unit = source.__index__ == 7 ? 2 : 4
    order = Encoding::LIST[source.__index__ == 7 ? 3 : 5]
    unread = 0
    at = 0
    while at < text.bytesize
      bytes = text.byteslice(at, unit).b
      at += bytes.bytesize
      if source.__index__ == 7
        return [Encoding::UTF_16LE, at, unread] if bytes == "\xFF\xFE".b
        return [Encoding::UTF_16BE, at, unread] if bytes == "\xFE\xFF".b
      else
        return [Encoding::UTF_32LE, at, unread] if bytes == "\xFF\xFE\x00\x00".b
        return [Encoding::UTF_32BE, at, unread] if bytes == "\x00\x00\xFE\xFF".b
      end
      stop = bytes.bytesize < unit ? :incomplete : :invalid
      # CRuby's only converter from either goes to UTF-8.
      raise __invalid_error__(stop, bytes, "".b, source, target, Encoding::UTF_8) unless options[:invalid] == :replace
      unread += 1
    end
    [order, at, unread]
  end

  def self.__escape_xml__(text, mode)
    # Built in bytes, so a dummy's ASCII is escaped too. Measured.
    escaped = String.new(encoding: Encoding::BINARY)
    escaped << "\"" if mode == :attr
    text.each_char do |c|
      c = c.b
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
    escaped.force_encoding(text.encoding)
  end

  # `to` is where the first converter goes, which is the step that failed.
  def self.__invalid_error__(stop, error, readagain, source, target, to)
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
    Encoding::InvalidByteSequenceError.new(message).__set__(error, readagain, source, to, stop == :incomplete)
  end

  # The character a converter had no spelling for, in the encoding it came
  # from: UTF-8's, by its codepoint, or the bytes it was given.
  def self.__error_char__(code, error, from)
    code ? Integer.__utf8__(code) : error.dup.force_encoding(from)
  end

  # A codepoint names itself as `U+00E9`; a character with none — a BINARY
  # byte, a JIS character Unicode lacks — as its bytes, in the encoding the
  # converter that failed came from. A conversion of more than one step says
  # so, and names the step that failed. Measured.
  def self.__undefined_error__(code, error, from, to, source, target)
    shown = code.nil? ? error.inspect : "U+#{Integer.__hex__(code, 4)}"
    message =
      if from.equal?(source) && to.equal?(target)
        "#{shown} from #{source.name} to #{target.name}"
      else
        "#{shown} to #{to.name} in conversion from #{__path__(source, target).map(&:name).join(" to ")}"
      end
    Encoding::UndefinedConversionError.new(message).__set__(__error_char__(code, error, from), from, to)
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
      step = __transcode__([__encoding_index__], start, 0, 0, false)
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
    # The newline decorators by CRuby's names, in the order it names them.
    NEWLINE_FLAGS = {
      "universal_newline" => UNIVERSAL_NEWLINE_DECORATOR, "crlf_newline" => CRLF_NEWLINE_DECORATOR,
      "cr_newline" => CR_NEWLINE_DECORATOR, "lf_newline" => LF_NEWLINE_DECORATOR
    }.freeze

    attr_reader :source_encoding, :destination_encoding, :last_error

    def self.__encoding__(value)
      return value if value.is_a?(Encoding)
      Encoding.find(String.__coerce__(value))
    end

    # The converters a conversion runs through, as pairs of encodings: see
    # `String.__path__`.
    def self.__path__(source, destination)
      String.__path__(source, destination).each_cons(2).to_a
    end

    # The path with its decorators, which work where ASCII is ASCII: last,
    # or before the converter into a destination that is not ASCII-compatible.
    # Measured.
    def self.__decorated__(path, decorators, destination)
      return path + decorators if destination.ascii_compatible?
      path[0...-1] + decorators + path[-1..]
    end

    def self.search_convpath(source, destination, **options)
      source = __encoding__(source)
      destination = __encoding__(destination)
      # Which pairs CRuby has a path for is its converter table's question.
      unless String.__supported_target__(source) && String.__supported_target__(destination)
        __needs_char_table__
      end
      decorators = String.__newline_decorators__(options)
      __check_decorators__(source, destination, decorators)
      __decorated__(__path__(source, destination), decorators, destination)
    end

    # The newline decorators `flags` name.
    def self.__newline_decorators__(flags)
      NEWLINE_FLAGS.select { |_, flag| flags & flag != 0 }.keys
    end

    # CRuby has no converter from an encoding to itself, decorated or not, nor
    # one for two newline decorators. Measured.
    def self.__check_decorators__(source, destination, decorators)
      if source.equal?(destination)
        with = decorators.empty? ? "" : " with #{decorators.join(",")}"
        raise Encoding::ConverterNotFoundError, "code converter not found (#{source.name} to #{destination.name}#{with})"
      end
      raise String.__decorators_not_found__(source, destination, decorators, nil) if decorators.size > 1
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
      @flags = 0
      replacement = nil
      if options.is_a?(Integer)
        @flags = options
      else
        options = options.to_hash unless options.is_a?(Hash)
        @flags = @flags | INVALID_REPLACE if options[:invalid] == :replace
        @flags = @flags | UNDEF_REPLACE if options[:undef] == :replace
        @flags = @flags | UNDEF_HEX_CHARREF if options[:xml]
        String.__newline_decorators__(options).each { |name| @flags = @flags | NEWLINE_FLAGS[name] }
        replacement = options[:replace]
      end
      @decorators = Converter.__newline_decorators__(@flags)
      Converter.__check_decorators__(@source_encoding, @destination_encoding, @decorators)
      unless String.__supported_target__(@source_encoding) && String.__supported_target__(@destination_encoding)
        __needs_char_table__
      end
      # The default is not converted: "?" stays US-ASCII. Measured.
      if replacement.nil?
        @replacement = __default_replacement__
      else
        self.replacement = replacement
      end
      stages = String.__path__(@source_encoding, @destination_encoding)
      @path = stages.map(&:__index__)
      # The newline decorator works in `transcode::step`, as `String#encode`'s
      # does, unless it reads the source itself — UTF-8 into UTF-16 or UTF-32
      # — where it works on the bytes of each chunk before they are read.
      newline = @decorators.first
      @source_newline = newline if newline && stages.drop(1).none?(&:ascii_compatible?)
      @mode = @source_newline ? 0 : String::NEWLINE_MODES.fetch(newline, 0) << 2
      # ISO-2022-JP's, and a CR `universal_newline` holds, between calls.
      @state = 0
      @held = false
      # Decorated source bytes after an error, read before the next chunk.
      @decorated = "".b
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
      # Kept in the encoding output is inserted in: the destination, or the
      # ASCII-compatible one a UTF-16 or UTF-32 destination converts through.
      # A replacement that cannot be converted is refused, the old one kept.
      # Measured.
      target = Converter.asciicompat_encoding(@destination_encoding) || @destination_encoding
      unless value.encoding.equal?(target)
        # Raised outside the rescue, so it has no cause: CRuby's has none.
        value = begin
          value.encode(target)
        rescue EncodingError
          nil
        end
        raise Encoding::UndefinedConversionError, "replacement character setup failed" if value.nil?
      end
      @replacement = value
    end

    def convpath
      Converter.__decorated__(Converter.__path__(@source_encoding, @destination_encoding), @decorators, @destination_encoding)
    end

    def inspect
      with = @decorators.empty? ? "" : " with #{@decorators.join(",")}"
      "#<Encoding::Converter: #{@source_encoding.name} to #{@destination_encoding.name}#{with}>"
    end

    def primitive_errinfo
      @errinfo.dup
    end

    def putback(max = nil)
      taken = max.nil? ? @readagain : @readagain.byteslice(@readagain.bytesize - [max, @readagain.bytesize].min, max)
      @readagain = @readagain.byteslice(0, @readagain.bytesize - taken.bytesize)
      taken.force_encoding(@source_encoding)
    end

    # Inserted before the destination's encoder, so it switches ISO-2022-JP's
    # G0 as converted text does. Measured.
    def insert_output(text)
      text = String.__coerce__(text)
      @pending_output, @state =
        String.__insert__(@pending_output, text, @destination_encoding, @state) ||
        String.__not_inserted__(text, @destination_encoding)
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
      chunk = source.nil? ? "".b : source.b
      buffered = @readagain + @decorated
      @readagain = "".b
      @decorated = "".b
      source&.__splice__(0, source.bytesize, "")
      output = @pending_output
      @pending_output = "".b
      result = @source_newline ? __run_turns__(chunk, buffered, output, flags) : __run__(buffered + chunk, output, flags, source)
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

    # One run over `chunk` where the newline decorator reads the source
    # itself, its turns read as `String#encode` reads them, so a replacement
    # comes after what the decorator had written. `buffered` is what an
    # earlier run left decorated or begun. `universal_newline` holds a
    # trailing CR of partial input until it sees what follows, as CRuby's
    # does. Measured.
    def __run_turns__(chunk, buffered, output, flags)
      final = flags & PARTIAL_INPUT == 0
      turns, @held = String.__decorator_turns__(chunk, nil, @source_newline, @held, final)
      via = Encoding::LIST[@path.first]
      _, @state, buffered, step = String.__read_turns__(turns, buffered, @path, @state, final, output, via, @destination_encoding) do |stopped|
        __replacement_for__(stopped, flags)
      end
      if step.nil?
        # A character begun at the end of partial input waits for the rest.
        @readagain = buffered
        @errinfo = [final ? :finished : :source_buffer_empty, nil, nil, nil, nil]
        @last_error = nil
        return @errinfo[0]
      end
      @readagain = step[3].b
      # ponytail: decorated bytes are kept to be read next, not put back in
      # `source`, where they would be decorated again; CRuby puts back as
      # much as its decorator has not buffered, which differs by chunk. The
      # upgrade is a map from decorated offsets to the source's.
      @decorated = buffered
      __stopped__(step)
    end

    # One run over `input`, appending to `output`. Errors the flags replace
    # are replaced; the first one they do not is reported, its bytes and any
    # read-again bytes taken from the input, and what follows put back in
    # `source` for the caller. Input that is not partial ends the output, as
    # ISO-2022-JP's return to ASCII.
    def __run__(input, output, flags, source)
      start = 0
      loop do
        step = input.__transcode__(@path, start, @state, @mode, flags & PARTIAL_INPUT == 0)
        converted, stop, error, readagain, following = step
        @state = step[9]
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
        replace = __replacement_for__(step, flags)
        if replace
          output, @state =
            String.__insert__(output, replace, @destination_encoding, @state) ||
            String.__not_inserted__(replace, @destination_encoding)
          start = following
          next
        end
        @readagain = readagain.b
        source&.__splice__(0, 0, input.byteslice(following + readagain.bytesize, input.bytesize))
        return __stopped__(step)
      end
    end

    # What the flags put in for the error `step` stopped at, or nil.
    def __replacement_for__(step, flags)
      if step[1] == :undefined
        if flags & UNDEF_MASK == UNDEF_HEX_CHARREF && step[6]
          "&#x#{Integer.__hex__(step[6], 1)};"
        elsif flags & UNDEF_MASK == UNDEF_REPLACE
          @replacement
        end
      elsif flags & INVALID_MASK == INVALID_REPLACE
        @replacement
      end
    end

    # The error `step` stopped at, kept for `primitive_errinfo` and
    # `last_error`, and its result.
    def __stopped__(step)
      _, stop, error, readagain, _, code, _, from, to = step
      result = { invalid: :invalid_byte_sequence, incomplete: :incomplete_input, undefined: :undefined_conversion }[stop]
      from = Encoding::LIST[from]
      to = Encoding::LIST[to]
      if stop == :undefined
        @last_error = String.__undefined_error__(code, error, from, to, @source_encoding, @destination_encoding)
        @errinfo = [result, @last_error.source_encoding_name, @last_error.destination_encoding_name, @last_error.error_char.b, "".b]
      else
        @last_error = String.__invalid_error__(stop, error.b, readagain.b, @source_encoding, @destination_encoding, to)
        @errinfo = [result, @source_encoding.name, @last_error.destination_encoding_name, error.b, readagain.b]
      end
      result
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
