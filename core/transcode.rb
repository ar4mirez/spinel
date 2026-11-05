# `String#encode`, `#encode!` and `#scrub` (#19), driving the transcoder in
# `crates/spinel-vm/src/transcode.rs` one step at a time. The VM converts
# between the encodings whose mapping is arithmetic — UTF-8, US-ASCII,
# BINARY, ISO-8859-1, UTF-16 and UTF-32 — EUC-JP, Shift_JIS,
# Windows-31J and CP51932, whose mapping is CRuby's table (#378, #419),
# ISO-2022-JP and stateless-ISO-2022-JP (#404), and CP50220 and CP50221
# (#419), and refuses any other. This file finds the
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
    def destination_encoding_name = @__destination_encoding_name__ || @__destination_encoding__&.name
    # nil until a conversion sets it, then true or false. Measured.
    def incomplete_input? = @__incomplete__

    def __set__(error, readagain, source, destination, incomplete)
      @__error_bytes__ = error
      @__readagain_bytes__ = readagain
      @__source_encoding__ = source
      @__destination_encoding__ = destination
      @__destination_encoding_name__ = String.__converter_name__(source, destination)
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

  # The bit of `transcode::State` that says the source's BOM told its order.
  BOM_READ = 64

  # The encoding each converts to and from on the way to UTF-8, by index,
  # where it is not UTF-8 itself: ISO-2022-JP through stateless-ISO-2022-JP
  # and EUC-JP, CP50220 and CP50221 through CP51932. Measured.
  CONVERTS_THROUGH = { 85 => 79, 79 => 10, 87 => 81, 88 => 81 }.freeze

  # The encoding a stateful encoder reads what is inserted in, by index: see
  # `__insert__`.
  INSERTS_THROUGH = { 85 => 79, 87 => 81, 88 => 81 }.freeze

  # CP50220 and CP50221, by index.
  CP5022X = [87, 88].freeze

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
    String.__econv_opts__(options) unless options.empty?
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
    if target.equal?(source) && !decorated && options[:invalid] == :replace && !options[:replace].nil?
      String.__scrub_compatible__(self, String.__replacement_option__(options[:replace]))
    end
    # A dummy has no characters to replace, and with a decorator nothing is
    # converted to find one: it is kept as it is, and the decorators work on
    # its bytes. `xml:` escapes even when nothing is transcoded. Measured.
    if target.equal?(source) && (decorated || options[:invalid] != :replace || source.dummy?)
      result = xml ? String.__escape_xml__(text, xml) : text.dup
      return String.__newlines__(result, newlines.first).force_encoding(target)
    end
    String.__convert__(text, source, target, options, newlines.first)
  end

  # CRuby's `str_transcode0` to the same encoding, undecorated, with
  # `invalid: :replace`: `scrub`, whose `replace:` must suit the string —
  # ASCII where a character can be a byte, or else in the string's own
  # encoding — even a dummy's, which is then kept as it is. Measured.
  # ponytail: CRuby asks only of a string whose coderange it has not found
  # clean, which this VM does not keep: a dummy's is taken as never found,
  # and another's as found where it is valid. The upgrade is keeping it.
  def self.__scrub_compatible__(text, replacement)
    return unless text.encoding.dummy? || !text.valid_encoding?
    encoding = text.encoding
    compatible =
      if replacement.encoding.ascii_compatible? && replacement.ascii_only?
        !UNICODE_INDEXES.drop(1).include?(encoding.__index__)
      else
        replacement.encoding.equal?(encoding)
      end
    return if compatible
    raise Encoding::CompatibilityError,
          "incompatible character encodings: #{encoding.__shown__} and #{replacement.encoding.__shown__}"
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
    return xml if [nil, :text, :attr].include?(xml)
    raise ArgumentError, "unexpected value for xml option: #{xml}" if Symbol === xml
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
    unless source.equal?(target)
      conversion = "#{source.name} to #{target.name}"
      names = names.empty? ? conversion : "#{conversion} with #{names}"
    end
    Encoding::ConverterNotFoundError.new("code converter not found (#{names})")
  end

  # CRLF and CR become LF, byte by byte: an invalid byte is left for the
  # transcoder to report, not refused as a String operation would (#412).
  # No ASCII-compatible encoding has CR or LF as a trailing byte.
  def self.__universal_newline__(text)
    text.b.split("\r\n", -1).join("\n").gsub("\r", "\n").force_encoding(text.encoding)
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
      out = held ? "\r".b << piece.b : piece.b
      held = out.getbyte(-1) == 13
      out = out.byteslice(0, out.bytesize - 1) if held
      [out.include?("\r") ? __universal_newline__(out) : out, held]
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
    # To itself it is `scrub`, which opens no converter.
    __replacement_spelled__(replacement, source, target, [newline].compact, xml) unless options[:replace].nil? || source.equal?(target)
    return __converted__(text, source, target, options) if CP5022X.include?(source.__index__) && CP5022X.include?(target.__index__)
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
      state = BOM_READ
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
        raise __invalid_error__(stop, error, readagain, from, target, to) unless options[:invalid] == :replace
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
      step = text.__transcode__(path, start, state, mode, true)
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
    __with_bom__(out, bom, target)
  end

  # CRuby's `rb_econv_set_replacement` as the converter opens: a `replace:`
  # with no spelling where what is inserted goes in — the target, or the
  # ASCII-compatible encoding its encoder reads — fails the open, as
  # ConverterNotFoundError naming the conversion, before any character
  # needs it. Measured.
  def self.__replacement_spelled__(replacement, source, target, newlines, xml)
    via = Encoding::Converter.asciicompat_encoding(target) || target
    return if replacement.encoding.equal?(via)
    step = replacement.__transcode__(__path__(replacement.encoding, via).map(&:__index__), 0, 0, 0, true)
    # A replacement this VM cannot convert is left to be found out when one
    # is needed.
    return if step.nil? || step[1] == :done
    raise __decorators_not_found__(source, target, newlines, xml)
  end

  # CP50220 to CP50221 and back, as CRuby's `transcode_loop` converts every
  # pair: through `Encoding::Converter`. The encoder reads what the decoder
  # wrote as a byte stream, and what goes in for an error it reads after all
  # it has still to read, which only the converter's buffers keep. No
  # character is undefined on the way, so no fallback is asked. Measured.
  def self.__converted__(text, source, target, options)
    converter = Encoding::Converter.new(source, target, options)
    out = String.new(encoding: target)
    result = converter.primitive_convert(text.b, out)
    raise converter.last_error unless result == :finished
    out
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
  # stateless-ISO-2022-JP for ISO-2022-JP, CP51932 for CP50220 and CP50221 —
  # and then through the target's encoder, so it switches G0 as the text
  # around it does, joins the katakana CP50220 holds, and leaves the source's
  # decoder as it was. nil when `piece` has no spelling
  # in `target`.
  def self.__insert__(out, piece, target, state)
    return [out, state] if piece.empty?
    via = Encoding::LIST[INSERTS_THROUGH.fetch(target.__index__, target.__index__)]
    bytes = piece.b
    unless piece.encoding.equal?(via)
      path = __path__(piece.encoding, via).map(&:__index__)
      step = piece.__transcode__(path, 0, 0, 0, false)
      __needs_char_table__ if step.nil?
      return nil unless step[1] == :done
      bytes = step[0].b
    end
    unless via.equal?(target)
      step = bytes.__transcode__([via.__index__, target.__index__], 0, state, 0, false)
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
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 22, 39, 79, 81, 85, 87, 88].include?(encoding.__index__)
  end

  # Every encoding a conversion passes through, `source` to `target`, along
  # CRuby's converters as `search_convpath` finds them. They make a tree
  # under UTF-8, which every other encoding converts to and from but those
  # in `CONVERTS_THROUGH`, and Shift_JIS also converts to and from EUC-JP,
  # which is shorter to EUC-JP and what is under it. Between these the
  # shortest path is the only one: up from `source` to where the two meet
  # and down to `target`. A conversion to the same encoding is the encoding
  # alone. Measured.
  def self.__path__(source, target)
    return [source] if source.equal?(target)
    up = __up__(source)
    down = __up__(target)
    euc_jp = Encoding::LIST[10]
    up = [source, *__up__(euc_jp)] if source.__index__ == 39 && down.include?(euc_jp)
    down = [target, *__up__(euc_jp)] if target.__index__ == 39 && up.include?(euc_jp)
    meet = up.find { |encoding| down.include?(encoding) }
    up[0..up.index(meet)] + down[0...down.index(meet)].reverse
  end

  # `encoding` and the encodings above it, up to UTF-8.
  def self.__up__(encoding)
    chain = [encoding]
    chain << Encoding::LIST[CONVERTS_THROUGH.fetch(chain.last.__index__, 1)] until chain.last.equal?(Encoding::UTF_8)
    chain
  end

  # What `invalid: :replace` and `undef: :replace` substitute: `replace:`, or
  # U+FFFD for a Unicode target and "?" for any other.
  def self.__replacement__(target, options)
    replacement = options[:replace]
    return __replacement_option__(replacement) unless replacement.nil?
    __unicode__(target) ? "�" : "?"
  end

  # A `replace:` as a String, refused when it is broken in its own encoding.
  def self.__replacement_option__(replacement)
    replacement = String.__coerce__(replacement)
    raise ArgumentError, "replacement string is broken: #{replacement.dump} as #{replacement.encoding}" unless replacement.valid_encoding?
    replacement
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

  # The name CRuby's converter from `from` gives `to`, its encoding's but
  # for CP50220's and CP50221's decoders, which write `cp51932`. Measured.
  def self.__converter_name__(from, to)
    return nil if to.nil?
    !from.nil? && [87, 88].include?(from.__index__) && to.__index__ == 81 ? "cp51932" : to.name
  end

  # A codepoint names itself as `U+00E9`; a character with none — a BINARY
  # byte, a JIS character Unicode lacks — as its bytes, in the encoding the
  # converter that failed came from. A conversion of more than one step says
  # so, and names the step that failed, each encoding as the converter into
  # it names it. Measured.
  def self.__undefined_error__(code, error, from, to, source, target)
    shown = code.nil? ? error.inspect : "U+#{Integer.__hex__(code, 4)}"
    message =
      if from.equal?(source) && to.equal?(target)
        "#{shown} from #{source.name} to #{target.name}"
      else
        path = __path__(source, target)
        names = [path.first.name] + path.each_cons(2).map { |a, b| __converter_name__(a, b) }
        "#{shown} to #{to.name} in conversion from #{names.join(" to ")}"
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
    # The bit of `transcode::State` that `universal_newline` holds a CR in.
    HELD_CR = 32
    private_constant :HELD_CR

    # `asciicompat_encoding`'s answers, by index.
    ASCIICOMPAT = { 3 => 1, 4 => 1, 5 => 1, 6 => 1, 7 => 1, 8 => 1, 15 => 1, 78 => 22, 85 => 79, 87 => 81, 88 => 81, 99 => 100 }.freeze
    private_constant :ASCIICOMPAT
    # The bit of `__transcode__`'s decorators that stops it after output.
    STOP_AFTER_OUTPUT = 32
    private_constant :STOP_AFTER_OUTPUT
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
    # How many bytes `__run_whole__` reads at once with `AFTER_OUTPUT`, enough
    # for an escape sequence and the character after it. One longer goes round
    # again.
    AFTER_OUTPUT_BYTES = 16
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

    # The encoding `value` names, or the name as it is when it names none,
    # "internal" with no default internal among them, as CRuby's `econv_args`
    # keeps one, for `__converter_check__` (#592).
    def self.__encoding_or_name__(value)
      return value if value.is_a?(Encoding)
      Encoding.find(value) || value
    rescue ArgumentError
      value
    end

    # CRuby's `rb_econv_open` finds no converter for a name that is no
    # encoding, `""` among them. Measured.
    def self.__converter_check__(source, destination, flags)
      return if source.is_a?(Encoding) && destination.is_a?(Encoding)
      raise Encoding::ConverterNotFoundError, "code converter not found (#{__description__(source, destination, flags)})"
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
      flags = __option_flags__(options)
      source = String.__coerce__(source) unless source.is_a?(Encoding)
      destination = String.__coerce__(destination) unless destination.is_a?(Encoding)
      source = __encoding_or_name__(source)
      destination = __encoding_or_name__(destination)
      __converter_check__(source, destination, flags)
      # Which pairs CRuby has a path for is its converter table's question.
      unless String.__supported_target__(source) && String.__supported_target__(destination)
        __needs_char_table__
      end
      __check_decorators__(source, destination, flags)
      __decorated__(__path__(source, destination), __path_names__(flags), destination)
    end

    # The flags `options` ask for, as `new` reads them. `xml:` also writes an
    # undefined character as a reference. Measured.
    def self.__option_flags__(options)
      String.__econv_opts__(options)
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
      newlines = NEWLINE_FLAGS.count { |_, flag| flags & flag != 0 }
      escapes = flags & XML_TEXT_DECORATOR != 0 && flags & XML_ATTR_CONTENT_DECORATOR != 0
      return unless (!source.nil? && source.equal?(destination)) || newlines > 1 || escapes
      raise Encoding::ConverterNotFoundError, "code converter not found (#{__description__(source, destination, flags)})"
    end

    # CRuby's `econv_description`: the names, joined by " to " when both are
    # given, then the decorators after " with ", or the decorators alone for a
    # converter of no names, and "no-conversion" for one of neither. A side is
    # an Encoding, a name that is none, or nil for none. Measured.
    def self.__description__(source, destination, flags)
      names = [source, destination].map { |side| side.is_a?(Encoding) ? side.name : side.to_s }.reject(&:empty?).join(" to ")
      decorators = __decorator_names__(flags).join(",")
      return decorators.empty? ? "no-conversion" : decorators if names.empty?
      decorators.empty? ? names : "#{names} with #{decorators}"
    end

    # Whether `source` and `destination` are both `""`, which CRuby takes for
    # a converter of decorators alone: it has no encodings and converts
    # nothing, what `make_readconv` makes for an IO with a newline option and
    # no internal encoding (#550). Measured.
    def self.__decorators_alone__?(source, destination)
      source.is_a?(String) && source.empty? && destination.is_a?(String) && destination.empty?
    end

    # The ASCII-compatible encoding an encoding's encoder reads, as CRuby's
    # converter table has it: UTF-8 for UTF-16 and UTF-32, stateless-ISO-
    # 2022-JP for ISO-2022-JP and its KDDI variant for ISO-2022-JP-KDDI,
    # CP51932 for CP50220 and CP50221, ISO-8859-1 for IBM037, and UTF-8 for
    # CESU-8, though CESU-8 is ASCII-compatible itself. nil for ISO-2022-JP-2
    # and UTF-7, which CRuby converts to nothing, for any other, and for a
    # name that resolves to none, "internal" with no default internal among
    # them (#622). Measured, for every dummy (#419) and CESU-8 (#684).
    def self.asciicompat_encoding(encoding)
      encoding = __encoding__(encoding)
      return nil if encoding.nil?
      through = ASCIICOMPAT[encoding.__index__]
      through && Encoding::LIST[through]
    rescue ArgumentError
      nil
    end

    # The encoding a BOM'd UTF-16 or UTF-32 is converted in until its BOM
    # says otherwise: big-endian. Any other is converted in itself.
    def self.__big_endian__(encoding)
      { 7 => Encoding::UTF_16BE, 8 => Encoding::UTF_32BE }.fetch(encoding.__index__, encoding)
    end

    def initialize(source, destination, options = 0)
      replacement = nil
      if options.is_a?(Integer)
        @flags = options
      else
        options = options.to_hash unless options.is_a?(Hash)
        @flags = Converter.__option_flags__(options)
        replacement = options[:replace]
      end
      source = String.__coerce__(source) unless source.is_a?(Encoding)
      destination = String.__coerce__(destination) unless destination.is_a?(Encoding)
      if Converter.__decorators_alone__?(source, destination)
        @source_encoding = @destination_encoding = nil
      else
        source = Converter.__encoding_or_name__(source)
        destination = Converter.__encoding_or_name__(destination)
        Converter.__converter_check__(source, destination, @flags)
        @source_encoding = source
        @destination_encoding = destination
      end
      Converter.__check_decorators__(@source_encoding, @destination_encoding, @flags)
      unless @source_encoding.nil? || (String.__supported_target__(@source_encoding) && String.__supported_target__(@destination_encoding))
        __needs_char_table__
      end
      # The default is not converted: "?" stays US-ASCII. Measured. A
      # replacement with no spelling where it goes in fails the open, as
      # CRuby's `rb_econv_open_opts` does, naming the conversion. Measured.
      if replacement.nil?
        @replacement = __default_replacement__
      else
        spelled = begin
          self.replacement = replacement
          true
        rescue Encoding::UndefinedConversionError
          false
        end
        raise Encoding::ConverterNotFoundError, "code converter not found (#{Converter.__description__(@source_encoding, @destination_encoding, @flags)})" unless spelled
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
      # Whether a call has stopped at `__econv_convert__`'s bound, short of
      # its limit.
      @stopped_short = false
      # Whether the last call stopped there, with more to write.
      @suspended = false
      # Whether a call stops there however many have before (#611).
      @goes_on = false
    end

    # A call past `__econv_convert__`'s bound stops there however many have
    # before, so the next goes on with what CRuby's goes on to: `IO#gets`
    # with a limit takes that much of a conversion that never ends (#611).
    # ponytail: going on is that loop's call a character, about 110us a byte
    # made on a release build, so `gets(140000)` takes 15s where CRuby's takes
    # 16ms. The upgrade is writing what the errors repeat without a call of
    # the transcoders for each.
    def __goes_on__=(goes_on)
      @goes_on = goes_on
    end

    # Whether the last call stopped at the bound, with more to write.
    def __suspended__? = @suspended

    # With no destination, "?" in BINARY. Measured.
    def __default_replacement__
      return "?".b if @destination_encoding.nil?
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
      # ASCII-compatible one a UTF-16 or UTF-32 destination converts through,
      # and with no destination as it is. A replacement that cannot be
      # converted is refused, the old one kept. Measured.
      target = __insert_encoding__
      unless target.nil? || value.encoding.equal?(target)
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
      return Converter.__path_names__(@flags) if @source_encoding.nil?
      Converter.__decorated__(Converter.__path__(@source_encoding, @destination_encoding), Converter.__path_names__(@flags), @destination_encoding)
    end

    # In BINARY, as CRuby's. Measured.
    def inspect
      "#<Encoding::Converter: #{Converter.__description__(@source_encoding, @destination_encoding, @flags)}>".b
    end

    # The transcoders a conversion runs, in order, as CRuby's
    # `rb_econv_elem_t`s: each converter of the path, and the decorators
    # last, or before the encoder into a destination that is not
    # ASCII-compatible, each with a buffer of its own that the next reads.
    # A BOM'd UTF-16 or UTF-32 source is read in units until its BOM tells
    # its order, and a BOM'd destination is written big-endian after a BOM
    # its first character writes. A converter of decorators alone has only
    # them, and one of nothing has none. Measured.
    def __elements__
      stages = @source_encoding.nil? ? [] : String.__path__(@source_encoding, @destination_encoding)
      elements = stages.each_cons(2).map do |from, to|
        Element.new(
          path: [Converter.__big_endian__(from).__index__, Converter.__big_endian__(to).__index__],
          src: from,
          dst: to,
          # An encoder takes what is inserted into the buffer it reads, as
          # CRuby's `asciicompat_encoder`.
          encoder: from.ascii_compatible? && !to.ascii_compatible?,
          # A stateful decoder reads an escape sequence with a function,
          # which waits for room although it writes nothing. Measured.
          fun: [85, 87, 88].include?(from.__index__),
          # A stateful encoder goes back to ASCII as it finishes, CP50220
          # after the katakana it holds.
          finisher: [85, 87, 88].include?(to.__index__)
        )
      end
      unless elements.empty?
        elements.first.unit = { 7 => 2, 8 => 4 }[@source_encoding.__index__]
        elements.last.bom = { 7 => "\xFE\xFF".b, 8 => "\x00\x00\xFE\xFF".b }[@destination_encoding.__index__]
      end
      decorators = Converter.__path_names__(@flags).map do |name|
        newline = name == "universal_newline" || name == "lf_newline"
        # The attribute's quote takes what is inserted before it, as an
        # encoder does, so it opens before it. Measured.
        quote = name == "xml_attr_quote"
        Element.new(decorator: name, encoder: quote, fun: newline || quote, finisher: newline || quote, state: false)
      end
      return elements + decorators if elements.empty? || @destination_encoding.ascii_compatible?
      elements[0...-1] + decorators + elements[-1..]
    end

    def __unread__(buffer)
      buffer.bytes.bytesize - buffer.at
    end

    # Whether the converter holds nothing a new one would not: no part of a
    # character read, nothing to read again or still to write, nothing
    # finished or inserted, and no transcoder's state moved. `IO` reads a line
    # at a time again from such a one (#609).
    def __idle__?
      return false unless @num_finished == 0 && __unread__(@in_buf) == 0
      @elements.all? do |element|
        element.readbuf.empty? && element.readagain.empty? && element.writebuf.empty? &&
          __unread__(element.out) == 0 && element.phase == :read && !element.finishing &&
          (element.state == 0 || element.state == false)
      end
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
      elsif at >= 4096 && at * 2 >= bytes.bytesize
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
      return "".b if element.nil?
      again = element.readagain
      count = max.nil? ? again.bytesize : [max, again.bytesize].min
      element.readagain = again.byteslice(0, again.bytesize - count)
      again = again.byteslice(again.bytesize - count, count)
      @source_encoding.nil? ? again.b : again.force_encoding(@source_encoding)
    end

    # Encoded to the encoding it is inserted in first, as `String#encode`
    # does, so what that cannot spell raises its error even once finished;
    # text already in it is taken as it is, however invalid. With no
    # destination that is BINARY. Measured.
    def insert_output(text)
      text = String.__coerce__(text).encode(__insert_encoding__ || Encoding::BINARY)
      __insert__(text.b)
      nil
    end

    # The encoding `insert_output` inserts in: the destination's, or the
    # ASCII-compatible one its encoder reads, as CRuby's
    # `rb_econv_encoding_to_insert_output`, or nil with no destination.
    def __insert_encoding__
      return nil if @destination_encoding.nil?
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
      if last.nil?
        @in_buf.bytes << bytes
        return
      end
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
      result = @elements.empty? ? __copy__(input, output, bytesize, flags) : __econv_convert__(input, output, bytesize, flags)
      __stopped__(result)
      source&.__splice__(0, 0, input.bytes.byteslice(input.at, __unread__(input)))
      destination.__splice__(offset, destination.bytesize - offset, "") unless offset.nil?
      destination.__splice__(destination.bytesize, 0, output)
      destination.force_encoding(@destination_encoding) unless @destination_encoding.nil?
      result
    end

    # CRuby's converter of nothing: what was inserted, then the source, as it
    # is, no further than `limit`. With `AFTER_OUTPUT` it stops after what was
    # inserted, or else a byte. It never finishes for good. Measured.
    def __copy__(input, output, limit, flags)
      after = flags & AFTER_OUTPUT != 0
      [@in_buf, input].each do |buffer|
        count = __unread__(buffer)
        count = 1 if after && count > 1 && buffer.equal?(input)
        room = __room__(output, limit)
        if room < count
          output << __read__(buffer, room)
          return :destination_buffer_full
        end
        output << __read__(buffer, count)
        return :after_output if after && count > 0
      end
      flags & PARTIAL_INPUT != 0 ? :source_buffer_empty : :finished
    end

    # CRuby's `rb_econv_convert`: an error the flags replace has what goes
    # in for it inserted, and the conversion goes on.
    # ponytail: CRuby never ends where each error breaks what goes in for
    # the next — a replacement after a character begun ahead of the encoder
    # (U+FFFD after "\xC3"), or one that is itself invalid — inserting more
    # than it reads. Past a bound the call is run again from where it began
    # with such a replacement before that character, and past it again it
    # stops at the error, unreplaced. The first call with a limit to pass the
    # bound still writing stops there instead, answering as though the limit
    # had come, so what it wrote is what CRuby's writes, and a line that ends
    # in it is CRuby's; CRuby's would go on to the limit, a call of this loop
    # a character. The calls after it run again as above, so a read of an IO
    # ends where CRuby's never does, but where `__goes_on__` has each stop
    # there too, for `IO#gets` with a limit (#611). The upgrade is whatever
    # CRuby settles on.
    def __econv_convert__(input, output, limit, flags)
      @suspended = false
      saved = first = fed = grown = nil
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
          grown = output.bytesize
        elsif inserted > 2 * fed + 64
          return result if begun
          if limit && (!@stopped_short || @goes_on) && output.bytesize > grown
            @stopped_short = @suspended = true
            @error_element = nil
            __insert__(bytes)
            return :destination_buffer_full
          end
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
          room = limit - output.bytesize - __buffered__ - 1
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
        after_output = flags & AFTER_OUTPUT != 0
        if read.empty?
          return :after_output if after_output && output.bytesize != start
          if limit.nil? && element.unit.nil? && __unread__(input) != 0 && !(after_output && element.decorator)
            # With nowhere to stop between characters, they are read all at
            # once, up to the first that writes anything with `AFTER_OUTPUT`.
            result = __run_whole__(element, input, output, after_output)
            return result if result
            next
          end
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
    # without bound, or up to the first character that writes anything with
    # `after_output`, which reads a few bytes at a time so that stopping costs
    # the same however much input is left: a character the bytes end in the
    # middle of waits begun, and an error is the result, or nil.
    def __run_whole__(element, input, output, after_output)
      if element.decorator
        output << __decorated_all__(element, __read__(input, __unread__(input)))
        return nil
      end
      bytes = input.bytes
      at = input.at
      if after_output
        bytes = bytes.__byteslice__(at, AFTER_OUTPUT_BYTES)
        at = 0
      end
      step = bytes.__transcode__(element.path, at, element.state, after_output ? STOP_AFTER_OUTPUT : 0, false)
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
        __skip__(input, step[4] - at)
        nil
      when :incomplete
        __skip__(input, bytes.bytesize - at)
        element.readbuf = step[2].b
        nil
      else
        __skip__(input, step[4] - at + step[3].bytesize)
        __element_error__(element, step)
      end
    end

    # What the decorator `element` writes for all of `bytes`, as
    # `__decorated__` does a byte at a time: along BINARY alone, where every
    # byte is a character kept as it is, `transcode::step` decorates them
    # with what the state holds of a CR.
    def __decorated_all__(element, bytes)
      mode =
        case element.decorator
        when "xml_text_escape" then String.__xml_mode__(:text, false)
        when "xml_attr_content_escape" then String.__xml_mode__(:attr, false)
        when "xml_attr_quote"
          return bytes.b if element.state || bytes.empty?
          element.state = true
          return "\"".b + bytes
        else String::NEWLINE_MODES.fetch(element.decorator) << 2
        end
      step = bytes.__transcode__([0], 0, element.state ? HELD_CR : 0, mode, false)
      element.state = step[9] & HELD_CR != 0
      step[0].b
    end

    # The step the transcoder `element` takes over the character it has read
    # so far, or nil while it needs more; with `finish`, no more comes.
    def __recognized__(element, finish)
      read = element.readbuf
      return [__decorated__(element, read), :done] if element.decorator
      unit = element.unit
      if unit
        # Before its BOM, a BOM'd UTF-16 or UTF-32 reads a unit at a time:
        # any other is invalid, and one cut short incomplete, as one is after
        # it too. Measured.
        return nil if read.bytesize < unit && !finish
        order = read.bytesize == unit ? BOM_ORDERS[read] : nil
        if order
          element.unit = nil
          element.path = [order.__index__, element.path[1]]
          element.state |= String::BOM_READ
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
        @errinfo = [result, from.name, String.__converter_name__(from, to), @last_error.error_char.b, "".b]
      else
        readagain = @error_element.readagain
        @last_error = String.__invalid_error__(stop, error.b, readagain.b, from, to, to)
        @errinfo = [result, from.name, String.__converter_name__(from, to), error.b, readagain.b]
      end
    end

    # Converts a chunk, raising the error a stop names; input that ends
    # mid-character waits for more, or for `finish`. A finished converter
    # converts nothing more. Measured.
    def convert(text)
      text = String.__coerce__(text)
      source = text.dup
      out = __new_out__
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

    # What `convert` and `finish` write into: BINARY with no destination.
    # Measured.
    def __new_out__
      @destination_encoding.nil? ? String.new : String.new(encoding: @destination_encoding)
    end

    def finish
      out = __new_out__
      result = primitive_convert("".b, out)
      raise @last_error if result == :invalid_byte_sequence || result == :incomplete_input || result == :undefined_conversion
      out
    end
  end
end
