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
    xml = String.__xml_option__(options)
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

  # `xml:`, which is nil, `:text` or `:attr`: `false` is refused. Measured.
  def self.__xml_option__(options)
    xml = options[:xml]
    return xml if xml.nil? || xml == :text || xml == :attr
    raise ArgumentError, "unexpected value for xml option: #{xml}" if xml.is_a?(Symbol)
    raise ArgumentError, "unexpected value for xml option"
  end

  # How the VM names the xml decorators: see `transcode::step`. `escape` is
  # nil, `:text` or `:attr`, an attribute's content, and `quote` whether
  # the attribute's quote goes around it, as `xml: :attr` has it.
  def self.__xml_mode__(escape, quote)
    { text: 1, attr: 2 }.fetch(escape, 0) | (quote ? 16 : 0)
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
  # decorators read the source itself (UTF-8 into UTF-16 or UTF-32), whether
  # `universal_newline` holds a CR after them, and whether the attribute's
  # quote is open. `escape` and `quote` are the xml decorators, as
  # `__xml_mode__` takes them. CRuby's first decorator writes one source
  # byte's worth and stops, and the encoder reads all of it before the next
  # turn. A CR `universal_newline` holds writes nothing: as the first
  # decorator it goes on to the next byte, but behind `xml:` that turn hands
  # on nothing. A `final` last turn is what finishing writes: the
  # attribute's closing quote, a held CR's LF. Measured.
  def self.__decorator_turns__(text, escape, quote, newline, held, quoted, final)
    xml = escape || quote
    turns = text.b.bytes.map do |byte|
      piece = escape ? __xml_entity__(byte.chr.b, escape).b : byte.chr.b
      next piece if !quote || quoted
      quoted = true
      "\"".b + piece
    end
    if final
      turns << (quote ? (quoted ? "\"".b : "\"\"".b) : "".b)
      quoted = false
    end
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
    [decorated, held, quoted]
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
  # that, the errors buffer more than they read, and with it the step that
  # keeps doing so, unreplaced.
  def self.__read_turns_in__(turns, buffered, path, state, final, out, via, named, begun)
    index = 0
    fed = buffered.bytesize
    inserted = 0
    again = false
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
        if inserted > 2 * fed + 64
          return nil unless begun
          return [out, state, rest + turns.drop(index).join.b, step]
        end
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
  # gains its BOM with the first character written, so a conversion that
  # writes nothing has none. Measured. A source of either reads its BOM to
  # learn its order. The VM runs the decorators — `xml:` and the newline
  # options — where CRuby's work, after a stateful source is decoded and
  # before a stateful target or one that is not ASCII-compatible is encoded,
  # and keeps the conversion's state between steps — ISO-2022-JP's G0, the
  # attribute's quote, a CR `universal_newline` holds — so what is inserted
  # for an error goes through it too.
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
    step_source = source
    start = 0
    unread = 0
    if source.__index__ == 7 || source.__index__ == 8
      step_source, start, unread = __bom_order__(text, source, target, options)
    end
    stages = __path__(step_source, step_target)
    path = stages.map(&:__index__)
    mode = __xml_mode__(xml, xml == :attr) | NEWLINE_MODES.fetch(newline, 0) << 2
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
        return __with_bom__(out, bom, target)
      end
      turns, = __decorator_turns__(text, xml, xml == :attr, newline, false, false, true)
      out, state = __read_turns__(turns, "".b, path, state, true, out, step_source, target) { |step| piece_for.call(step)[0] }
      out << "".__transcode__(path, 0, state, 0, true)[0].b
      return __with_bom__(out, bom, target)
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
    __with_bom__(out, bom, target)
  end

  # `out` in `target`, after `bom` when anything was written.
  def self.__with_bom__(out, bom, target)
    out.prepend(bom) if bom && !out.empty?
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
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 22, 39, 79, 85].include?(encoding.__index__)
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
    text.each_char { |c| escaped << __xml_entity__(c.b, mode) }
    escaped << "\"" if mode == :attr
    escaped.force_encoding(text.encoding)
  end

  # The entity `c` is escaped as, as text or as an attribute's content, or
  # `c` itself. Measured.
  def self.__xml_entity__(c, escape)
    case c
    when "&" then "&amp;"
    when "<" then "&lt;"
    when ">" then "&gt;"
    when "\"" then escape == :attr ? "&quot;" : c
    when "'" then escape == :attr ? "&apos;" : c
    else c
    end
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
    # The xml decorators by CRuby's names, in the order it names them after
    # the newline ones, and by the names a path gives them.
    XML_FLAGS = {
      "xml_text" => XML_TEXT_DECORATOR, "xml_attr_content" => XML_ATTR_CONTENT_DECORATOR,
      "xml_attr_quote" => XML_ATTR_QUOTE_DECORATOR
    }.freeze
    XML_PATH_NAMES = {
      "xml_text" => "xml_text_escape", "xml_attr_content" => "xml_attr_content_escape",
      "xml_attr_quote" => "xml_attr_quote"
    }.freeze
    # Bytes read from the front.
    class Buffer
      attr_accessor :bytes, :at

      def initialize(bytes = "".b)
        @bytes = bytes
        @at = 0
      end

      def copy
        copied = Buffer.new(@bytes.dup)
        copied.at = @at
        copied
      end
    end
    private_constant :Buffer

    # One transcoder: what it has read of the character it is in, the bytes
    # an error left it to read again, the output of a character, or of its
    # finishing, still to be written, and its buffer.
    class Element
      attr_accessor :path, :src, :dst, :decorator, :encoder, :fun, :finisher, :state, :unit, :bom,
                    :out, :readbuf, :readagain, :writebuf, :need_room, :phase, :finishing, :last, :error

      def initialize(path: nil, src: nil, dst: nil, decorator: nil, encoder: false, fun: false, finisher: false, state: 0)
        @path = path
        @src = src
        @dst = dst
        @decorator = decorator
        @encoder = encoder
        @fun = fun
        @finisher = finisher
        @state = state
        @unit = nil
        @bom = nil
        @out = Buffer.new
        @readbuf = "".b
        @readagain = "".b
        @writebuf = "".b
        @need_room = false
        @phase = :read
        @finishing = false
        @last = :source_buffer_empty
        @error = nil
      end
    end
    private_constant :Element

    # The most `__windowed__` reads at once.
    WINDOW = 4096
    # The least room in the destination that is worth a window.
    WINDOW_ROOM = 8
    # A BOM'd UTF-16's and UTF-32's BOMs, by the order they name.
    BOM_ORDERS = {
      "\xFF\xFE".b => Encoding::UTF_16LE, "\xFE\xFF".b => Encoding::UTF_16BE,
      "\xFF\xFE\x00\x00".b => Encoding::UTF_32LE, "\x00\x00\xFE\xFF".b => Encoding::UTF_32BE
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
      flags = __option_flags__(options)
      __check_decorators__(source, destination, flags)
      __decorated__(__path__(source, destination), __path_names__(flags), destination)
    end

    # The flags `options` ask for, as `new` reads them. `xml:` also writes an
    # undefined character as a reference. Measured.
    def self.__option_flags__(options)
      flags = 0
      flags = flags | INVALID_REPLACE if options[:invalid] == :replace
      flags = flags | UNDEF_REPLACE if options[:undef] == :replace
      xml = String.__xml_option__(options)
      flags = flags | UNDEF_HEX_CHARREF | XML_TEXT_DECORATOR if xml == :text
      flags = flags | UNDEF_HEX_CHARREF | XML_ATTR_CONTENT_DECORATOR | XML_ATTR_QUOTE_DECORATOR if xml == :attr
      String.__newline_decorators__(options).each { |name| flags = flags | NEWLINE_FLAGS[name] }
      flags
    end

    # The decorators `flags` name, by CRuby's names: the newline ones, then
    # the xml ones. Measured.
    def self.__decorator_names__(flags)
      NEWLINE_FLAGS.merge(XML_FLAGS).select { |_, flag| flags & flag != 0 }.keys
    end

    # The decorators `flags` name in a path, which runs the xml ones first.
    # Measured.
    def self.__path_names__(flags)
      xml = XML_FLAGS.select { |_, flag| flags & flag != 0 }.keys.map { |name| XML_PATH_NAMES[name] }
      xml + NEWLINE_FLAGS.select { |_, flag| flags & flag != 0 }.keys
    end

    # CRuby has no converter from an encoding to itself, decorated or not, nor
    # one for two newline decorators, or for both xml escapes. Measured.
    def self.__check_decorators__(source, destination, flags)
      names = __decorator_names__(flags)
      newlines = NEWLINE_FLAGS.count { |_, flag| flags & flag != 0 }
      escapes = flags & XML_TEXT_DECORATOR != 0 && flags & XML_ATTR_CONTENT_DECORATOR != 0
      return unless source.equal?(destination) || newlines > 1 || escapes
      with = names.empty? ? "" : " with #{names.join(",")}"
      raise Encoding::ConverterNotFoundError, "code converter not found (#{source.name} to #{destination.name}#{with})"
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

    # The encoding a BOM'd UTF-16 or UTF-32 is converted in until its BOM
    # says otherwise: big-endian. Any other is converted in itself.
    def self.__big_endian__(encoding)
      { 7 => Encoding::UTF_16BE, 8 => Encoding::UTF_32BE }.fetch(encoding.__index__, encoding)
    end

    def initialize(source, destination, options = 0)
      @source_encoding = Converter.__encoding__(source)
      @destination_encoding = Converter.__encoding__(destination)
      replacement = nil
      if options.is_a?(Integer)
        @flags = options
      else
        options = options.to_hash unless options.is_a?(Hash)
        @flags = Converter.__option_flags__(options)
        replacement = options[:replace]
      end
      Converter.__check_decorators__(@source_encoding, @destination_encoding, @flags)
      unless String.__supported_target__(@source_encoding) && String.__supported_target__(@destination_encoding)
        __needs_char_table__
      end
      # The default is not converted: "?" stays US-ASCII. Measured.
      if replacement.nil?
        @replacement = __default_replacement__
      else
        self.replacement = replacement
      end
      @elements = __elements__
      # What `insert_output` puts before an encoder that reads the source
      # itself, read before the source, as CRuby's `in_buf`.
      @in_buf = Buffer.new
      # How many bytes `__windowed__` reads at once.
      @window = WINDOW
      # How many transcoders, from the first, have finished.
      @num_finished = 0
      @errinfo = [:source_buffer_empty, nil, nil, nil, nil]
      @last_error = nil
      # The transcoder whose error the last conversion reported.
      @error_element = nil
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
      @replacement_bytes = nil
    end

    def convpath
      Converter.__decorated__(Converter.__path__(@source_encoding, @destination_encoding), Converter.__path_names__(@flags), @destination_encoding)
    end

    # In BINARY, as CRuby's. Measured.
    def inspect
      names = Converter.__decorator_names__(@flags)
      with = names.empty? ? "" : " with #{names.join(",")}"
      "#<Encoding::Converter: #{@source_encoding.name} to #{@destination_encoding.name}#{with}>".b
    end

    # The transcoders a conversion runs, in order, as CRuby's
    # `rb_econv_elem_t`s: each converter of the path, and the decorators
    # last, or before the encoder into a destination that is not
    # ASCII-compatible, each with a buffer of its own that the next reads.
    # A BOM'd UTF-16 or UTF-32 source is read in units until its BOM tells
    # its order, and a BOM'd destination is written big-endian after a BOM
    # its first character writes. Measured.
    def __elements__
      stages = String.__path__(@source_encoding, @destination_encoding)
      elements = stages.each_cons(2).map do |from, to|
        Element.new(
          path: [Converter.__big_endian__(from).__index__, Converter.__big_endian__(to).__index__],
          src: from,
          dst: to,
          # An encoder takes what is inserted into the buffer it reads, as
          # CRuby's `asciicompat_encoder`.
          encoder: from.ascii_compatible? && !to.ascii_compatible?,
          # ISO-2022-JP's decoder reads an escape sequence with a function,
          # which waits for room although it writes nothing. Measured.
          fun: from.__index__ == 85,
          # ISO-2022-JP's encoder goes back to ASCII as it finishes.
          finisher: to.__index__ == 85
        )
      end
      elements.first.unit = { 7 => 2, 8 => 4 }[@source_encoding.__index__]
      elements.last.bom = { 7 => "\xFE\xFF".b, 8 => "\x00\x00\xFE\xFF".b }[@destination_encoding.__index__]
      decorators = Converter.__path_names__(@flags).map do |name|
        newline = name == "universal_newline" || name == "lf_newline"
        # The attribute's quote takes what is inserted before it, as an
        # encoder does, so it opens before it. Measured.
        quote = name == "xml_attr_quote"
        Element.new(decorator: name, encoder: quote, fun: newline || quote, finisher: newline || quote, state: false)
      end
      return elements + decorators if @destination_encoding.ascii_compatible?
      elements[0...-1] + decorators + elements[-1..]
    end

    def __unread__(buffer)
      buffer.bytes.bytesize - buffer.at
    end

    # The next `count` bytes of `buffer`, read.
    def __read__(buffer, count)
      taken = buffer.bytes.__byteslice__(buffer.at, count)
      __skip__(buffer, taken.bytesize)
      taken
    end

    # Past the next `count` bytes of `buffer`.
    def __skip__(buffer, count)
      bytes = buffer.bytes
      at = buffer.at + count
      if at == bytes.bytesize
        buffer.bytes = "".b
        at = 0
      elsif at >= 4096
        buffer.bytes = bytes.byteslice(at, bytes.bytesize - at)
        at = 0
      end
      buffer.at = at
    end

    def __room__(output, limit)
      limit.nil? ? Float::INFINITY : limit - output.bytesize
    end

    def primitive_errinfo
      @errinfo.dup
    end

    # The bytes an error left the first transcoder to read again, from the
    # end, which it then does not, as CRuby's `rb_econv_putback`: whatever
    # the last call answered, but never those a later transcoder holds.
    # Measured.
    def putback(max = nil)
      element = @elements.first
      again = element.readagain
      count = max.nil? ? again.bytesize : [max, again.bytesize].min
      element.readagain = again.byteslice(0, again.bytesize - count)
      again.byteslice(again.bytesize - count, count).force_encoding(@source_encoding)
    end

    # Encoded to the encoding it is inserted in first, as `String#encode`
    # does, so what that cannot spell raises its error even once finished;
    # text already in it is taken as it is, however invalid. Measured.
    def insert_output(text)
      text = String.__coerce__(text).encode(__insert_encoding__)
      __insert__(text.b)
      nil
    end

    # The encoding `insert_output` inserts in: the destination's, or the
    # ASCII-compatible one its encoder reads, as CRuby's
    # `rb_econv_encoding_to_insert_output`.
    def __insert_encoding__
      Converter.asciicompat_encoding(@destination_encoding) || @destination_encoding
    end

    # `bytes` inserted as CRuby's `rb_econv_insert_output` inserts them: into
    # the buffer the last transcoder reads where it is an encoder, the bytes
    # an error left it to read again after them, or else into its own, to be
    # written before anything it writes next. Measured. With `begun`, bytes
    # that do not start with ASCII go before what the encoder has still to
    # read when that ends in a character begun: see `__econv_convert__`.
    def __insert__(bytes, begun = false)
      return if bytes.empty?
      last = @elements.last
      unless last.encoder
        last.out.bytes << bytes
        return
      end
      buffer = @elements.size == 1 ? @in_buf : @elements[-2].out
      rest = buffer.bytes.byteslice(buffer.at, __unread__(buffer)) if begun && last.path && bytes.getbyte(0) >= 0x80
      if rest && String.__begun__(rest, last.path)
        buffer.bytes = bytes + last.readagain + rest
        buffer.at = 0
      else
        buffer.bytes << bytes << last.readagain
      end
      last.readagain = "".b
    end

    # Converts as much of `source` as it can into `destination` and says why
    # it stopped, as CRuby's: every transcoder reads what the one before it
    # wrote, a character at a time, and an error, or the destination filling,
    # is reported where it happens. Consumed bytes leave `source`. Measured.
    def primitive_convert(source, destination, offset = nil, bytesize = nil, options = nil)
      Kernel.__check_frozen__(destination)
      flags = 0
      if options.is_a?(Integer)
        flags = options
      elsif options.is_a?(Hash)
        flags = flags | PARTIAL_INPUT if options[:partial_input]
        flags = flags | AFTER_OUTPUT if options[:after_output]
      end
      offset = Integer.__index__(offset) unless offset.nil?
      bytesize = Integer.__index__(bytesize) unless bytesize.nil?
      raise ArgumentError, "output_byteoffset too big" if !offset.nil? && offset > destination.bytesize
      input = Buffer.new(source.nil? ? "".b : source.b)
      source&.__splice__(0, source.bytesize, "")
      output = "".b
      result = __econv_convert__(input, output, bytesize, flags)
      __stopped__(result)
      source&.__splice__(0, 0, input.bytes.byteslice(input.at, __unread__(input)))
      destination.__splice__(offset, destination.bytesize - offset, "") unless offset.nil?
      destination.__splice__(destination.bytesize, 0, output)
      destination.force_encoding(@destination_encoding)
      result
    end

    # CRuby's `rb_econv_convert`: an error the flags replace has what goes
    # in for it inserted, and the conversion goes on.
    # ponytail: CRuby never ends where each error breaks what goes in for
    # the next — a replacement after a character begun ahead of the encoder
    # (U+FFFD after "\xC3"), or one that is itself invalid — inserting more
    # than it reads. Past a bound the call is run again from where it began
    # with such a replacement before that character, and past it again it
    # stops at the error, unreplaced. The upgrade is whatever CRuby settles
    # on.
    def __econv_convert__(input, output, limit, flags)
      saved = first = fed = nil
      begun = false
      inserted = 0
      loop do
        result = __convert0__(input, output, limit, flags)
        bytes =
          case result
          when :invalid_byte_sequence, :incomplete_input
            __replacement_bytes__ if @flags & INVALID_MASK == INVALID_REPLACE
          when :undefined_conversion
            case @flags & UNDEF_MASK
            when UNDEF_REPLACE then __replacement_bytes__
            when UNDEF_HEX_CHARREF
              reference = @error_element.error[6]
              "&#x#{Integer.__hex__(reference, 1)};".b if reference
            end
          end
        return result if bytes.nil?
        inserted += 1
        if saved.nil?
          # Nothing goes differently before the first error replaced, so a
          # run again starts from it.
          saved = __saved__(input, output)
          first = bytes
          fed = __unread__(input) + __buffered__
        elsif inserted > 2 * fed + 64
          return result if begun
          __restore__(saved, input, output)
          begun = true
          inserted = 1
          bytes = first
        end
        __insert__(bytes, begun)
      end
    end

    # The replacement in the encoding it is inserted in, or nil when it has
    # no spelling there.
    def __replacement_bytes__
      @replacement_bytes ||= String.__in_encoding__(@replacement, __insert_encoding__)
    end

    # How many bytes wait inside, in every transcoder.
    def __buffered__
      @elements.sum do |element|
        __unread__(element.out) + element.readbuf.bytesize + element.readagain.bytesize + element.writebuf.bytesize
      end + __unread__(@in_buf)
    end

    # Everything a conversion changes, to run it again from where it began.
    def __saved__(input, output)
      elements = @elements.map do |element|
        copy = element.dup
        copy.out = element.out.copy
        copy.readbuf = element.readbuf.dup
        copy.readagain = element.readagain.dup
        copy.writebuf = element.writebuf.dup
        copy
      end
      [elements, @in_buf.copy, @num_finished, input.bytes, input.at, output.bytesize]
    end

    def __restore__(saved, input, output)
      @elements, @in_buf, @num_finished, input.bytes, input.at, size = saved
      output.__splice__(size, output.bytesize - size, "")
    end

    # CRuby's `rb_econv_convert0`: what was inserted after the last
    # transcoder is written first, then what was inserted before a lone
    # encoder is read, then the source; through more than one transcoder,
    # the first goes a character at a time, the rest reading on after it.
    def __convert0__(input, output, limit, flags)
      result, position = __convert0_run__(input, output, limit, flags)
      @error_element = position && __error__(result) ? @elements[position] : nil
      result
    end

    def __error__(result)
      result == :invalid_byte_sequence || result == :incomplete_input || result == :undefined_conversion
    end

    def __convert0_run__(input, output, limit, flags)
      waiting = @elements.last.out
      written = false
      unless __unread__(waiting) == 0
        room = __room__(output, limit)
        if room < __unread__(waiting)
          output << __read__(waiting, room)
          return [:destination_buffer_full, nil]
        end
        output << __read__(waiting, __unread__(waiting))
        written = true
      end
      unless __unread__(@in_buf) == 0
        result = __trans_conv__(@in_buf, output, limit, (flags & ~AFTER_OUTPUT) | PARTIAL_INPUT)
        return result unless result[0] == :source_buffer_empty
      end
      # After writing what waited, `AFTER_OUTPUT` reads nothing more of the
      # source, and goes on as though it had ended: unless partial, that
      # finishes, leaving the source; partial, it answers that it wrote.
      # Measured.
      if written && flags & AFTER_OUTPUT != 0 && __unread__(input) != 0
        result = __trans_conv__(Buffer.new, output, limit, flags)
        return result[0] == :source_buffer_empty ? [:after_output, nil] : result
      end
      return __trans_conv__(input, output, limit, flags) if flags & AFTER_OUTPUT != 0 || (@elements.size == 1 && limit.nil?)
      result = __windowed__(input, output, limit, flags)
      return result if result
      return __trans_conv__(input, output, limit, flags) if @elements.size == 1
      loop do
        result = __trans_conv__(input, output, limit, flags | AFTER_OUTPUT)
        return result unless result[0] == :after_output
      end
    end

    # Where nothing stops the conversion, and a byte of the destination is
    # left over, the first transcoder reading a window of the source at once
    # makes what reading it a character at a time makes; so it does if an
    # error stops the first transcoder itself, the rest having read all it
    # wrote before it. Any other stop puts the window back as it was, and nil
    # leaves it to be read a character at a time, so it stops where it should.
    # The window narrows after one put back and widens after one that is not,
    # so what is put back stays near the distance to the next stop. Once the
    # first transcoder has finished the source is not read, and the rest go
    # on finishing however much of it is left.
    def __windowed__(input, output, limit, flags)
      return nil if @num_finished > 0
      loop do
        size = __unread__(input)
        size = @window if size > @window
        unless limit.nil?
          room = limit - output.bytesize - 1
          return nil if room < WINDOW_ROOM
          size = room if size > room
        end
        saved = __saved__(input, output)
        window = Buffer.new(input.bytes.__byteslice__(input.at, size))
        more = __unread__(input) > size
        result = __trans_conv__(window, output, nil, more ? flags | PARTIAL_INPUT : flags)
        stopped = result[0]
        unless (stopped == :source_buffer_empty || stopped == :finished || (__error__(stopped) && result[1] == 0)) &&
               (limit.nil? || output.bytesize < limit)
          __restore__(saved, input, output)
          @window = [size / 16, 16].max
          return nil
        end
        @window = [@window * 2, WINDOW].min
        __skip__(input, size - __unread__(window))
        return result unless more && stopped == :source_buffer_empty
      end
    end

    # CRuby's `rb_trans_conv`: sweeps the transcoders from past the last one
    # with something to report, and answers for the last one that did not
    # want more input, and where it is.
    def __trans_conv__(input, output, limit, flags)
      last = @elements.size - 1
      @elements[0].last = :source_buffer_empty if @elements[0].last == :after_output
      start = last
      start -= 1 while start >= 0 && !__reports__(@elements[start].last) && @elements[start].last != :finished
      if start < 0
        if @elements[last].last == :destination_buffer_full && flags & AFTER_OUTPUT != 0
          result = __trans_conv__(Buffer.new, output, limit, (flags & ~AFTER_OUTPUT) | PARTIAL_INPUT)
          return result[0] == :source_buffer_empty ? [:after_output, nil] : result
        end
      end
      start += 1
      loop do
        reported = __trans_sweep__(input, output, limit, flags, start)
        break if reported == -1 || reported == last
        start = reported + 1
      end
      last.downto(0) do |at|
        result = @elements[at].last
        next if result == :source_buffer_empty
        @elements[at].last = :source_buffer_empty if __reports__(result) && result != :after_output
        return [result, at]
      end
      [:source_buffer_empty, nil]
    end

    def __reports__(result)
      result == :invalid_byte_sequence || result == :incomplete_input || result == :undefined_conversion || result == :after_output
    end

    # CRuby's `trans_sweep`: each transcoder from `start` reads what the one
    # before it wrote, until none moves; the first that has something to
    # report stops it. Input is partial for one until all before it have
    # finished. The index of the one that stopped it, or -1.
    def __trans_sweep__(input, output, limit, flags, start)
      last = @elements.size - 1
      moved = true
      while moved
        moved = false
        at = start
        while at <= last
          element = @elements[at]
          source = at == 0 ? input : @elements[at - 1].out
          sink = at == last ? output : element.out.bytes
          f = flags
          f = f | PARTIAL_INPUT unless @num_finished == at
          if at == 0 && flags & AFTER_OUTPUT != 0
            start = 1
            flags = flags & ~AFTER_OUTPUT
          end
          f = f & ~AFTER_OUTPUT unless at == 0
          unread = __unread__(source)
          size = sink.bytesize
          result = element.last = __transcoding__(element, source, sink, at == last ? limit : nil, f)
          moved = true if __unread__(source) != unread || sink.bytesize != size
          return at if __reports__(result)
          @num_finished = at + 1 if result == :finished
          at += 1
        end
      end
      -1
    end

    # One call of the transcoder `element`, as CRuby's
    # `transcode_restartable`: what an error left it to read again is read
    # first, as partial input.
    def __transcoding__(element, input, output, limit, flags)
      again = element.readagain
      unless again.empty?
        element.readagain = "".b
        again = Buffer.new(again)
        result = __transcoding_run__(element, again, output, limit, flags | PARTIAL_INPUT)
        unless result == :source_buffer_empty
          element.readagain = element.readagain + again.bytes.byteslice(again.at, __unread__(again))
          return result
        end
      end
      __transcoding_run__(element, input, output, limit, flags)
    end

    # CRuby's `transcode_restartable0`: reads `input` a byte at a time until
    # a character is whole, then writes what it makes into `output`, no
    # further than `limit` bytes. A character is read before its first byte
    # is written, so the one that does not fit has left the input and waits
    # inside, as does one the input ends in the middle of where more may
    # come; output a function makes, even none, waits for a byte of room;
    # finishing writes what the transcoder still owes. With `AFTER_OUTPUT`
    # it stops after the first character that writes anything. Measured.
    def __transcoding_run__(element, input, output, limit, flags)
      partial = flags & PARTIAL_INPUT != 0
      start = output.bytesize
      loop do
        return :finished if element.phase == :finished
        if element.phase == :write
          if element.need_room
            return :destination_buffer_full if __room__(output, limit) < 1
            element.need_room = false
          end
          pending = element.writebuf
          until pending.empty?
            room = __room__(output, limit)
            return :destination_buffer_full if room < 1
            room = pending.bytesize if room > pending.bytesize
            output << pending.byteslice(0, room)
            pending = element.writebuf = pending.byteslice(room, pending.bytesize - room)
          end
          element.phase = element.finishing ? :finished : :read
          next
        end
        read = element.readbuf
        if read.empty? && limit.nil? && flags & AFTER_OUTPUT == 0 && element.unit.nil? && __unread__(input) != 0
          # With nowhere to stop between characters, they are read all at
          # once.
          result = __run_whole__(element, input, output)
          return result if result
          next
        end
        if read.empty?
          return :after_output if flags & AFTER_OUTPUT != 0 && output.bytesize != start
          if __unread__(input) == 0
            return :source_buffer_empty if partial
            __finish_element__(element)
            next
          end
        end
        if __unread__(input) == 0
          return :source_buffer_empty if partial
          # The input ends inside a character.
          step = __recognized__(element, true)
        else
          read << __read__(input, 1)
          step = __recognized__(element, false)
          next if step.nil?
        end
        return __element_error__(element, step) unless step[1] == :done
        __written__(element, step[0].b)
      end
    end

    # `__transcoding_run__` over all of `input` at once, into an `output`
    # without bound: a character the input ends in the middle of waits begun,
    # and an error is the result, or nil.
    def __run_whole__(element, input, output)
      if element.decorator
        output << __decorated_all__(element, __read__(input, __unread__(input)))
        return nil
      end
      at = input.at
      step = input.bytes.__transcode__(element.path, at, element.state, 0, false)
      __needs_char_table__ if step.nil?
      element.state = step[9]
      out = step[0].b
      if element.bom && !out.empty?
        out = element.bom + out
        element.bom = nil
      end
      output << out
      case step[1]
      when :done
        __skip__(input, __unread__(input))
        nil
      when :incomplete
        __skip__(input, __unread__(input))
        element.readbuf = step[2].b
        nil
      else
        __skip__(input, step[4] - at + step[3].bytesize)
        __element_error__(element, step)
      end
    end

    # What the decorator `element` writes for all of `bytes`, as
    # `__decorated__` does a byte at a time.
    def __decorated_all__(element, bytes)
      name = element.decorator
      case name
      when "xml_text_escape", "xml_attr_content_escape"
        escape = name == "xml_attr_content_escape" ? :attr : :text
        bytes.b.gsub(/[&<>"']/n) { |c| String.__xml_entity__(c, escape) }.b
      when "xml_attr_quote"
        return bytes.b if element.state || bytes.empty?
        element.state = true
        "\"".b + bytes
      when "crlf_newline", "cr_newline" then String.__newlines__(bytes, name).b
      else
        out, element.state = String.__newline_turn__(bytes, name, element.state)
        out
      end
    end

    # The step the transcoder `element` takes over the character it has read
    # so far, or nil while it needs more; with `finish`, no more comes.
    def __recognized__(element, finish)
      read = element.readbuf
      return [__decorated__(element, read), :done] if element.decorator
      unit = element.unit
      if unit
        # Before its BOM, a BOM'd UTF-16 or UTF-32 reads a unit at a time:
        # any other is invalid, and one cut short incomplete. Measured.
        return nil if read.bytesize < unit && !finish
        order = read.bytesize == unit ? BOM_ORDERS[read] : nil
        if order
          element.unit = nil
          element.path = [order.__index__, element.path[1]]
          return ["".b, :done]
        end
        return ["".b, read.bytesize < unit ? :incomplete : :invalid, read.dup, "".b]
      end
      step = read.__transcode__(element.path, 0, element.state, 0, finish)
      __needs_char_table__ if step.nil?
      return nil if step[1] == :incomplete && !finish
      element.state = step[9]
      step
    end

    # What the decorator `element` writes for the byte `byte`, as CRuby's
    # decorators read a byte at a time.
    def __decorated__(element, byte)
      name = element.decorator
      case name
      when "xml_text_escape" then String.__xml_entity__(byte, :text).b
      when "xml_attr_content_escape" then String.__xml_entity__(byte, :attr).b
      when "xml_attr_quote"
        return byte.b if element.state
        element.state = true
        "\"".b + byte
      when "crlf_newline", "cr_newline" then String.__newlines__(byte, name).b
      else
        out, element.state = String.__newline_turn__(byte, name, element.state)
        out
      end
    end

    # The character `element` has read makes `out`, written next. A BOM'd
    # destination's BOM goes before its first.
    def __written__(element, out)
      element.readbuf = "".b
      if element.bom && !out.empty?
        out = element.bom + out
        element.bom = nil
      end
      element.writebuf = out
      element.need_room = !out.empty? || element.fun
      element.phase = :write
    end

    # What `element` writes as it finishes: the attribute's closing quote,
    # the LF of a CR `universal_newline` holds, ISO-2022-JP's return to
    # ASCII. Even nothing waits for a byte of room. Measured.
    def __finish_element__(element)
      element.finishing = true
      unless element.finisher
        element.phase = :finished
        return
      end
      out =
        case element.decorator
        when "xml_attr_quote" then element.state ? "\"".b : "\"\"".b
        when nil then "".__transcode__(element.path, 0, element.state, 0, true)[0].b
        else element.state ? "\n".b : "".b
        end
      element.state = false if element.decorator
      element.writebuf = out
      element.need_room = true
      element.phase = :write
    end

    # The error `step` stopped `element` at: the bytes after it are read
    # again, and its result.
    def __element_error__(element, step)
      element.readbuf = "".b
      element.readagain = step[3].b
      element.error = step
      { invalid: :invalid_byte_sequence, incomplete: :incomplete_input, undefined: :undefined_conversion }[step[1]]
    end

    # What `result` leaves for `primitive_errinfo` and `last_error`: with an
    # error, the transcoder's that reported it, named by its encodings, as
    # CRuby's. Measured.
    def __stopped__(result)
      @last_error = nil
      @errinfo = [result, nil, nil, nil, nil]
      return unless @error_element
      _, stop, error, _, _, code = @error_element.error
      from = @error_element.src
      to = @error_element.dst
      if stop == :undefined
        @last_error = String.__undefined_error__(code, error, from, to, @source_encoding, @destination_encoding)
        @errinfo = [result, from.name, to.name, @last_error.error_char.b, "".b]
      else
        readagain = @error_element.readagain
        @last_error = String.__invalid_error__(stop, error.b, readagain.b, from, to, to)
        @errinfo = [result, from.name, to.name, error.b, readagain.b]
      end
    end

    # Converts a chunk, raising the error a stop names; input that ends
    # mid-character waits for more, or for `finish`. A finished converter
    # converts nothing more. Measured.
    def convert(text)
      text = String.__coerce__(text)
      source = text.dup
      out = String.new(encoding: @destination_encoding)
      loop do
        result = primitive_convert(source, out, nil, nil, partial_input: true)
        case result
        when :source_buffer_empty then return out
        when :finished then raise ArgumentError, "converter already finished"
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
