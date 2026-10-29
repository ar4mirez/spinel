# StringIO — an IO over a String (#56).
#
# CRuby's is a C extension, `ext/stringio/stringio.c`; this is the same
# behaviour in Ruby, which is what a built-in gem's C half becomes in Spinel
# when Ruby can say it (`docs/engine.md`, "Built-in gems"). It is loaded by
# `require "stringio"`, not at boot.
#
# The state is the String, a byte position, a line number, and which of
# reading and writing are open. Positions are bytes, as an IO's are: reading
# takes bytes from the String and gives them its encoding, and writing
# overwrites bytes at the position, padding with NULs past the end.
class StringIO
  include Enumerable
  include IO::Readable if defined?(IO::Readable)
  include IO::Writable if defined?(IO::Writable)

  # CRuby's is a data object, so `Marshal.dump` refuses one.
  Marshal.__opaque_class__(self)

  VERSION = "3.2.0"
  MAX_LENGTH = 2**63 - 1

  # The newline `puts` writes: US-ASCII, as CRuby's `rb_default_rs`, so a
  # US-ASCII StringIO does not convert it.
  NEWLINE = "\n".encode(Encoding::US_ASCII).freeze
  private_constant :NEWLINE

  def self.new(*args, **options, &block)
    warn("StringIO::new() does not take block; use StringIO::open() instead", uplevel: 1) unless block.nil?
    super(*args, **options)
  end

  def self.open(*args, **options, &block)
    io = new(*args, **options)
    return io unless block
    begin
      yield io
    ensure
      io.instance_variable_set(:@__string__, nil)
      io.close unless io.closed?
    end
  end

  def initialize(string = nil, mode = (alone = true; nil), **options)
    alone &&= !string.nil?
    if string.nil?
      string = String.new(encoding: Encoding.default_external)
      mode ||= "r+"
    else
      string = String.__coerce__(string) unless string.is_a?(String)
    end
    __set_mode__(string, mode, options)
    # A String given alone fixes the encoding to its own, over any keyword,
    # as CRuby's `strio_init` does: a later `force_encoding` of the String
    # does not change what is read or what a write converts into.
    @__encoding__ = string.encoding if alone
    @__string__ = string
    @__pos__ = 0
    @__lineno__ = 0
    @__unget__ = nil
    self
  end
  private :initialize

  def initialize_copy(other)
    super
    self
  end

  # The mode, as a String, an Integer of `File::Constants` flags, or nil —
  # which is read-write for a String that can be written and read-only for a
  # frozen one.
  def __set_mode__(string, mode, options)
    if !mode.nil? && !options[:mode].nil?
      raise ArgumentError, "mode specified twice"
    end
    mode = options[:mode] if mode.nil?
    binmode = options[:binmode]
    textmode = options[:textmode]
    if binmode && textmode
      raise ArgumentError, "both textmode and binmode specified"
    end
    readable = true
    writable = !string.frozen?
    append = false
    truncate = false
    encoding = nil
    flagged_binary = nil
    unless mode.nil?
      if mode.is_a?(Integer)
        access = mode & 3
        readable = access == IO::RDONLY || access == IO::RDWR
        writable = access == IO::WRONLY || access == IO::RDWR
        append = mode & IO::APPEND != 0
        truncate = mode & IO::TRUNC != 0
      else
        mode = String.__coerce__(mode)
        letters, external, _internal = mode.split(":", 3)
        letters ||= ""
        plus = letters.include?("+")
        case letters[0]
        when "r"
          readable = true
          writable = plus
        when "w"
          readable = plus
          writable = true
          truncate = true
        when "a"
          readable = plus
          writable = true
          append = true
        else
          raise ArgumentError, "invalid access mode " + mode
        end
        flagged_binary = true if letters.include?("b")
        flagged_binary = false if letters.include?("t")
        unless external.nil? || external.empty?
          if options.key?(:encoding) || options.key?(:external_encoding) || options.key?(:internal_encoding)
            raise ArgumentError, "encoding specified twice"
          end
          encoding = external.start_with?("BOM|") ? external[4..] : external
        end
      end
    end
    unless flagged_binary.nil?
      if (options.key?(:binmode) && !binmode.nil?) || (options.key?(:textmode) && !textmode.nil?)
        raise ArgumentError, flagged_binary ? "binmode specified twice" : "textmode specified twice"
      end
    end
    if writable && string.frozen?
      raise FrozenError.new("can't modify frozen String", receiver: string) if truncate && !readable && mode.is_a?(Integer)
      raise Errno::EACCES
    end
    raise FrozenError.new("can't modify frozen String", receiver: string) if truncate && string.frozen?
    string.replace(String.new(encoding: string.encoding)) if truncate
    encoding = options[:encoding] || options[:external_encoding] || encoding
    binary = binmode || flagged_binary
    @__encoding__ = if binary
      Encoding::BINARY
    elsif encoding
      Encoding.find(encoding.to_s)
    end
    @__readable__ = @__opened_readable__ = readable
    @__writable__ = @__opened_writable__ = writable
    @__append__ = append
  end
  private :__set_mode__

  def reopen(*args, **options)
    if args.size == 1 && options.empty? && !args[0].is_a?(String) && args[0].respond_to?(:to_strio)
      args = [args[0].to_strio]
    end
    if args.size == 1 && options.empty? && args[0].is_a?(StringIO)
      other = args[0]
      @__string__ = other.string
      @__pos__ = other.pos
      @__lineno__ = other.lineno
      @__readable__ = !other.closed_read?
      @__writable__ = !other.closed_write?
      @__append__ = false
      @__encoding__ = nil
      return self
    end
    initialize(*args, **options)
  end

  # -- the String ------------------------------------------------------------

  def string
    @__string__
  end

  def string=(string)
    string = String.__coerce__(string) unless string.is_a?(String)
    @__string__ = string
    @__pos__ = 0
    @__lineno__ = 0
    @__readable__ = true
    @__writable__ = !string.frozen?
    @__append__ = false
    string
  end

  def size
    __check_open__
    @__string__.bytesize
  end
  alias length size

  def external_encoding
    @__encoding__ || (@__string__.nil? ? Encoding.default_external : @__string__.encoding)
  end

  def internal_encoding
    nil
  end

  def set_encoding(external, _internal = nil, **_options)
    encoding = external.nil? ? Encoding.default_external : (external.is_a?(Encoding) ? external : Encoding.find(external.to_s.split(":")[0]))
    @__encoding__ = encoding
    @__string__.force_encoding(encoding) if @__writable__ && !@__string__.frozen?
    self
  end

  def set_encoding_by_bom
    boms = { Encoding::UTF_8 => "\xEF\xBB\xBF".b, Encoding::UTF_32BE => "\x00\x00\xFE\xFF".b,
             Encoding::UTF_32LE => "\xFF\xFE\x00\x00".b, Encoding::UTF_16BE => "\xFE\xFF".b,
             Encoding::UTF_16LE => "\xFF\xFE".b }
    head = @__string__.byteslice(0, 4).b
    boms.each do |encoding, mark|
      next unless head.start_with?(mark)
      @__pos__ = mark.bytesize
      set_encoding(encoding)
      return encoding
    end
    nil
  end

  # -- open and closed -------------------------------------------------------

  def close
    @__readable__ = false
    @__writable__ = false
    nil
  end

  def close_read
    raise IOError, "closing non-duplex IO for reading" unless @__opened_readable__
    @__readable__ = false
    nil
  end

  def close_write
    raise IOError, "closing non-duplex IO for writing" unless @__opened_writable__
    @__writable__ = false
    nil
  end

  def closed?
    !@__readable__ && !@__writable__
  end

  def closed_read?
    !@__readable__
  end

  def closed_write?
    !@__writable__
  end

  def __check_open__
    raise IOError, "not opened" if @__string__.nil?
  end
  private :__check_open__

  def __readable!
    raise IOError, "not opened for reading" unless @__readable__ && !@__string__.nil?
  end
  private :__readable!

  def __writable!
    raise IOError, "not opened for writing" unless @__writable__ && !@__string__.nil?
  end
  private :__writable!

  # -- position --------------------------------------------------------------

  def pos
    __check_open__
    @__pos__
  end
  alias tell pos

  def pos=(position)
    __check_open__
    position = Integer.__index__(position)
    raise Errno::EINVAL, "pos" if position.negative?
    @__pos__ = position
  end

  def seek(offset, whence = IO::SEEK_SET)
    raise IOError, "closed stream" if closed?
    offset = Integer.__index__(offset)
    whence = case whence
    when :SET, :set then IO::SEEK_SET
    when :CUR, :cur then IO::SEEK_CUR
    when :END, :end then IO::SEEK_END
    else Integer.__index__(whence)
    end
    base = case whence
    when IO::SEEK_SET then 0
    when IO::SEEK_CUR then @__pos__
    when IO::SEEK_END then @__string__.bytesize
    else raise Errno::EINVAL, "invalid whence"
    end
    raise Errno::EINVAL if base + offset < 0
    @__pos__ = base + offset
    0
  end

  def rewind
    __check_open__
    @__pos__ = 0
    @__lineno__ = 0
    0
  end

  def eof?
    __readable!
    @__pos__ >= @__string__.bytesize
  end
  alias eof eof?

  def lineno
    @__lineno__
  end

  def lineno=(number)
    @__lineno__ = Integer.__index__(number)
  end

  # -- IO's answers for something that is not a file -------------------------

  def binmode
    @__encoding__ = Encoding::BINARY
    @__string__.force_encoding(Encoding::BINARY) if @__writable__ && !@__string__.frozen?
    self
  end

  def fcntl(*)
    raise NotImplementedError, "fcntl() function is unimplemented on this machine"
  end

  def fileno = nil
  def flush = self
  def fsync = 0
  def isatty = false
  alias tty? isatty
  def pid = nil
  def sync = true
  def sync=(value)
    value
  end

  def path
    nil
  end

  def inspect
    "#<StringIO:0x" + __address__ + ">"
  end

  # -- reading ---------------------------------------------------------------

  # The bytes from the position to the end, in the String's encoding.
  def __rest__
    @__string__.byteslice(@__pos__, @__string__.bytesize - @__pos__) || ""
  end
  private :__rest__

  def __encoded__(bytes)
    bytes.force_encoding(external_encoding)
  end
  private :__encoded__

  def read(length = nil, buffer = nil)
    __readable!
    unless buffer.nil?
      buffer = String.__coerce__(buffer) unless buffer.is_a?(String)
      raise FrozenError.new("can't modify frozen String: " + buffer.inspect, receiver: buffer) if buffer.frozen?
    end
    if length.nil?
      text = @__pos__ >= @__string__.bytesize ? "" : __rest__
      @__pos__ = [@__pos__, @__string__.bytesize].max
      @__pos__ = @__string__.bytesize if @__pos__ < @__string__.bytesize
      text = __encoded__(text.dup)
      return text if buffer.nil?
      encoding = buffer.encoding
      buffer.replace(text)
      buffer.force_encoding(encoding)
      return buffer
    end
    length = Integer.__index__(length)
    raise ArgumentError, "negative length " + length.to_s + " given" if length.negative?
    if length.zero?
      buffer&.replace("")
      return buffer || "".b
    end
    if @__pos__ >= @__string__.bytesize
      buffer&.replace("")
      return nil
    end
    bytes = @__string__.byteslice(@__pos__, length).b
    @__pos__ += bytes.bytesize
    return bytes if buffer.nil?
    encoding = buffer.encoding
    buffer.replace(bytes)
    buffer.force_encoding(encoding)
    buffer
  end

  def pread(length, offset, buffer = nil)
    __readable!
    length = Integer.__index__(length)
    offset = Integer.__index__(offset)
    raise ArgumentError, "negative string size (or size too big): " + length.to_s if length.negative?
    raise Errno::EINVAL, "pread: Invalid offset argument: " + offset.to_s if offset.negative?
    if length.zero?
      return buffer.nil? ? "".b : buffer.replace("")
    end
    raise EOFError, "end of file reached" if offset >= @__string__.bytesize
    bytes = @__string__.byteslice(offset, length).b
    return bytes if buffer.nil?
    buffer.replace(bytes)
  end

  def sysread(*args)
    text = read(*args)
    raise EOFError, "end of file reached" if text.nil? || (text.empty? && !(args[0].is_a?(Integer) && args[0].zero?) && !args.empty? && !args[0].nil?)
    text
  end
  alias readpartial sysread

  def read_nonblock(length, buffer = nil, exception: true)
    text = read(length, buffer)
    if text.nil? || (text.empty? && length != 0)
      return nil unless exception
      raise EOFError, "end of file reached"
    end
    text
  end

  def getc
    __readable!
    return nil if @__pos__ >= @__string__.bytesize
    rest = __encoded__(__rest__.dup)
    char = rest[0]
    char = rest.byteslice(0, 1) if char.nil? || char.empty?
    @__pos__ += char.bytesize
    char
  end

  def getbyte
    __readable!
    return nil if @__pos__ >= @__string__.bytesize
    byte = @__string__.getbyte(@__pos__)
    @__pos__ += 1
    byte
  end

  def readchar
    char = getc
    raise EOFError, "end of file reached" if char.nil?
    char
  end

  def readbyte
    byte = getbyte
    raise EOFError, "end of file reached" if byte.nil?
    byte
  end

  def each_char
    return to_enum(:each_char) unless block_given?
    while (char = getc)
      yield char
    end
    self
  end

  def each_byte
    return to_enum(:each_byte) unless block_given?
    while (byte = getbyte)
      yield byte
    end
    self
  end

  def each_codepoint
    return to_enum(:each_codepoint) unless block_given?
    __readable!
    while (char = getc)
      yield char.ord
    end
    self
  end

  # Push bytes back before the position: over what was read when they fit
  # there, and in front of the String when they do not.
  def ungetc(text)
    __readable!
    return nil if text.nil?
    text = text.chr(external_encoding) if text.is_a?(Integer)
    text = String.__coerce__(text) unless text.is_a?(String)
    __unget__(text.b)
  end

  def ungetbyte(byte)
    __readable!
    return nil if byte.nil?
    bytes = byte.is_a?(Integer) ? (byte & 0xff).chr.b : String.__coerce__(byte).b
    __unget__(bytes)
  end

  def __unget__(bytes)
    return nil if bytes.empty?
    raise IOError, "not modifiable string" if @__string__.frozen?
    size = bytes.bytesize
    length = @__string__.bytesize
    if @__pos__ > length
      __splice__(length, 0, "\0".b * (@__pos__ - length))
    end
    if @__pos__ >= size
      @__pos__ -= size
      __splice__(@__pos__, size, bytes)
    else
      __splice__(0, @__pos__, bytes)
      @__pos__ = 0
    end
    nil
  end
  private :__unget__

  # -- lines -----------------------------------------------------------------

  # `gets`'s arguments: a separator (nil for the rest, "" for paragraphs),
  # a byte limit, and `chomp:`.
  def __line_arguments__(args)
    separator = $/
    limit = nil
    case args.size
    when 0
    when 1
      first = args[0]
      if first.nil?
        separator = nil
      elsif first.is_a?(String)
        separator = first
      elsif first.respond_to?(:to_str)
        separator = first.to_str
      else
        limit = Integer.__index__(first)
      end
    when 2
      separator = args[0].nil? ? nil : String.__coerce__(args[0])
      limit = args[1].nil? ? nil : Integer.__index__(args[1])
    else
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
    end
    [separator, limit]
  end
  private :__line_arguments__

  def __line__(separator, limit, chomp)
    length = @__string__.bytesize
    return nil if @__pos__ >= length
    if limit&.zero?
      return __encoded__(+"")
    end
    start = @__pos__
    paragraph = separator == ""
    if paragraph
      # Blank lines before a paragraph are skipped, and after it swallowed.
      start += 1 while start < length && @__string__.getbyte(start) == 10
      return nil if start >= length
      separator = "\n\n"
    end
    stop =
      if separator.nil?
        length
      else
        found = @__string__.b.index(separator.b, start)
        found.nil? ? length : found + separator.bytesize
      end
    # A paragraph takes every newline that ends it. Measured.
    stop += 1 while paragraph && stop < length && @__string__.getbyte(stop) == 10
    if limit && limit.positive? && stop - start > limit
      stop = start + limit
      # Never part of a character: a limit that lands inside one takes it
      # whole.
      text = @__string__.byteslice(start, stop - start)
      while stop < length && !__encoded__(text.dup).valid_encoding? && stop - start < limit + 8
        stop += 1
        text = @__string__.byteslice(start, stop - start)
      end
    end
    line = __encoded__(@__string__.byteslice(start, stop - start).dup)
    @__pos__ = stop
    if chomp && !separator.nil?
      if paragraph
        line = line.sub(/\n+\z/, "")
      elsif line.end_with?(separator)
        line = line[0, line.length - separator.length]
        line = line.chomp("\r") if separator == "\n"
      end
    end
    line
  end
  private :__line__

  def gets(*args, chomp: false)
    __readable!
    separator, limit = __line_arguments__(args)
    line = __line__(separator, limit, chomp)
    unless line.nil?
      @__lineno__ += 1
      $. = @__lineno__
    end
    $_ = line
    line
  end

  def readline(*args, chomp: false)
    line = gets(*args, chomp: chomp)
    raise EOFError, "end of file reached" if line.nil?
    line
  end

  def each_line(*args, chomp: false, &block)
    return to_enum(:each_line, *args, chomp: chomp) if block.nil?
    __readable!
    separator, limit = __line_arguments__(args)
    raise ArgumentError, "invalid limit: 0 for each_line" if limit&.zero?
    while (line = __line__(separator, limit, chomp))
      @__lineno__ += 1
      yield line
    end
    self
  end
  alias each each_line

  def readlines(*args, chomp: false)
    __readable!
    separator, limit = __line_arguments__(args)
    raise ArgumentError, "invalid limit: 0 for readlines" if limit&.zero?
    lines = []
    while (line = __line__(separator, limit, chomp))
      @__lineno__ += 1
      lines.push(line)
    end
    lines
  end

  # -- writing ---------------------------------------------------------------

  # Replace `count` bytes at `at` with `bytes`, keeping the String's
  # encoding. ponytail: a copy of the whole String per write, where CRuby
  # writes in place; String has no `bytesplice` here yet.
  def __splice__(at, count, bytes)
    string = @__string__
    encoding = string.encoding
    whole = string.b
    head = whole.byteslice(0, at) || "".b
    tail = whole.byteslice(at + count, whole.bytesize - at - count) || "".b
    string.replace(head + bytes.b + tail)
    string.force_encoding(encoding)
  end
  private :__splice__

  def write(*objects)
    __writable!
    written = 0
    objects.each do |object|
      text = String === object ? object : object.to_s
      written += __write__(text)
    end
    written
  end

  def __write__(text)
    return 0 if text.empty?
    raise IOError, "not modifiable string" if @__string__.frozen?
    string = @__string__
    target = external_encoding
    source = text.encoding
    # As CRuby's `strio_write`: BINARY and US-ASCII are never converted into,
    # and a String that cannot be converted is kept as it is, which raises
    # `Encoding::CompatibilityError` unless it is BINARY or US-ASCII.
    unless target == source || target == Encoding::BINARY || target == Encoding::US_ASCII
      converted = __convert__(text, target)
      if converted.nil?
        unless source == Encoding::BINARY || source == Encoding::US_ASCII
          raise Encoding::CompatibilityError, "incompatible character encodings: #{target} and #{source}"
        end
      else
        text = converted
      end
    end
    length = string.bytesize
    @__pos__ = length if @__append__
    if @__pos__ == length && target != Encoding::BINARY && source != Encoding::BINARY
      # Appended as `String#<<` does, which can give the String the written
      # one's encoding or raise `Encoding::CompatibilityError`, as
      # `rb_str_buf_append` does.
      string << text
      written = string.bytesize - length
      @__pos__ += written
      return written
    end
    bytes = text.b
    if @__pos__ > length
      __splice__(length, 0, "\0".b * (@__pos__ - length))
      length = @__pos__
    end
    overwritten = [bytes.bytesize, length - @__pos__].min
    __splice__(@__pos__, overwritten, bytes)
    @__pos__ += bytes.bytesize
    bytes.bytesize
  end
  private :__write__

  # `text` in `encoding`, or nil when it cannot be converted, as CRuby's
  # `rb_str_conv_enc`: an ASCII-only String into an ASCII-compatible encoding
  # is relabelled rather than transcoded.
  def __convert__(text, encoding)
    if (encoding.ascii_compatible? && text.ascii_only?) || encoding == Encoding::BINARY
      return text.dup.force_encoding(encoding)
    end
    text.encode(encoding)
  rescue EncodingError
    nil
  end
  private :__convert__

  def syswrite(object)
    write(object)
  end

  def write_nonblock(object, exception: true)
    write(object)
  end

  def <<(object)
    write(object)
    self
  end

  def print(*objects)
    objects = [$_] if objects.empty?
    objects.each_with_index do |object, index|
      write($,) if index.positive? && !$,.nil?
      write(object.nil? ? "" : object.to_s)
    end
    write($\) unless $\.nil?
    nil
  end

  def printf(format, *args)
    write(Kernel.format(format, *args))
    nil
  end

  def puts(*objects)
    __writable!
    if objects.empty?
      write(NEWLINE)
      return nil
    end
    __puts__(objects, [])
    nil
  end

  def __puts__(objects, seen)
    objects.each do |object|
      if Array === object || (!(String === object) && object.respond_to?(:to_ary) && (array = object.to_ary).is_a?(Array))
        array ||= object
        if seen.any? { |s| s.equal?(array) }
          write("[...]".encode(Encoding::US_ASCII))
          write(NEWLINE)
          next
        end
        __puts__(array, seen + [array])
        next
      end
      text = object.nil? ? "" : (String === object ? object : object.to_s)
      text = Kernel.instance_method(:to_s).bind(object).call unless text.is_a?(String)
      write(text)
      write(NEWLINE) unless text.__end_with_newline__
    end
  end
  private :__puts__

  def putc(object)
    __writable!
    if String === object
      write(object[0] || "")
    else
      write((Integer.__index__(object) & 0xff).chr)
    end
    object
  end

  def truncate(length)
    __writable!
    length = Integer.__index__(length)
    raise Errno::EINVAL, "negative length" if length.negative?
    size = @__string__.bytesize
    if length < size
      __splice__(length, size - length, "".b)
    else
      __splice__(size, 0, "\0".b * (length - size))
    end
    0
  end
end
