# `zlib` (#59), at the version CRuby 4.0.9 ships (3.2.3), over zlib 1.3.1.
# CRuby's is `ext/zlib/zlib.c`, all C, over the system's libz. Here zlib.c is
# Ruby, function by function, and libz is `spinel-vm/src/zlib.rs`, a port of
# its deflate and inflate behind four primitives:
#
# - `__zlib_init__`: deflateInit2 or inflateInit2, a stream's state String;
# - `__zlib_run__`: zlib.c's `zstream_run_func`, deflate or inflate called
#   until the output buffer stops growing, as the C loop calls them;
# - `__zlib_ctl__`: the stream's other calls (reset, end, set_dictionary,
#   params, sync, sync_point, and its counters);
# - `__zlib_checksum__`: adler32 and crc32 over a String's bytes.
#
# What a program sees of zlib.c's output buffer is modelled, not just its
# bytes: how much room each deflate call is given is what sizes the blocks
# of an uncompressed (level 0) stream, and a block given to `deflate` or
# `inflate` is handed the buffer each time it reaches 16 KiB. So a Stream
# keeps the capacity CRuby's String would have (`rb_str_buf_new`,
# `rb_str_modify_expand`, an embedded String's slot), and its `avail_out`.

module Zlib
  VERSION = "3.2.3".b
  ZLIB_VERSION = "1.3.1".b

  class Error < StandardError; end
  class StreamEnd < Error; end
  class NeedDict < Error; end
  class DataError < Error; end
  class StreamError < Error; end
  class MemError < Error; end
  class BufError < Error; end
  class VersionError < Error; end
  class InProgressError < Error; end

  BINARY = 0
  ASCII = 1
  TEXT = 1
  UNKNOWN = 2

  NO_COMPRESSION = 0
  BEST_SPEED = 1
  BEST_COMPRESSION = 9
  DEFAULT_COMPRESSION = -1

  FILTERED = 1
  HUFFMAN_ONLY = 2
  RLE = 3
  FIXED = 4
  DEFAULT_STRATEGY = 0

  MAX_WBITS = 15
  DEF_MEM_LEVEL = 8
  MAX_MEM_LEVEL = 9

  NO_FLUSH = 0
  SYNC_FLUSH = 2
  FULL_FLUSH = 3
  FINISH = 4

  OS_CODE = 3
  OS_MSDOS = 0x00
  OS_AMIGA = 0x01
  OS_VMS = 0x02
  OS_UNIX = 0x03
  OS_ATARI = 0x05
  OS_OS2 = 0x06
  OS_MACOS = 0x07
  OS_TOPS20 = 0x0a
  OS_WIN32 = 0x0b
  OS_VMCMS = 0x04
  OS_ZSYSTEM = 0x08
  OS_CPM = 0x09
  OS_QDOS = 0x0c
  OS_RISCOS = 0x0d
  OS_UNKNOWN = 0xff

  # zlib's return codes.
  Z_OK__ = 0
  Z_STREAM_END__ = 1
  Z_NEED_DICT__ = 2
  Z_DATA_ERROR__ = -3
  Z_BUF_ERROR__ = -5

  # `zError`: what an error says when the stream left no message.
  ERROR_TEXT__ = {
    1 => "stream end", 2 => "need dictionary", 0 => "", -1 => "file error",
    -2 => "stream error", -3 => "data error", -4 => "insufficient memory",
    -5 => "buffer error", -6 => "incompatible version"
  }.freeze
  private_constant :Z_OK__, :Z_STREAM_END__, :Z_NEED_DICT__, :Z_DATA_ERROR__,
                   :Z_BUF_ERROR__, :ERROR_TEXT__

  # `raise_zlib_error`.
  def self.__raise__(err, msg)
    msg ||= ERROR_TEXT__.fetch(err, "")
    case err
    when 1 then raise StreamEnd, msg
    when 2 then raise NeedDict, msg
    when -2 then raise StreamError, msg
    when -3 then raise DataError, msg
    when -5 then raise BufError, msg
    when -6 then raise VersionError, msg
    when -4 then raise MemError, msg
    when -1 then raise SystemCallError.new(msg)
    else raise Error, "unknown zlib error #{err}: #{msg}"
    end
  end

  # `FIX2INT`, which every level, window size and flush goes through: an
  # Integer, a Float truncated, or anything with `to_int`, that fits an int.
  def self.__int__(value)
    if Float === value
      if value.nan? || value >= 2147483648.0 || value < -2147483648.0
        raise RangeError, "float #{format("%-.10g", value)} out of range of integer"
      end
      return value.to_i
    end
    IO.__c_int__(value)
  end

  # `NUM2ULONG`: a negative long wraps, as C's conversion does.
  def self.__ulong__(value)
    value = value.to_i if Float === value
    value = Integer.__index__(value)
    raise RangeError, "bignum too big to convert into 'unsigned long'" if value > 18446744073709551615
    raise RangeError, "bignum out of range of unsigned long" if value < -9223372036854775808
    value < 0 ? value + 18446744073709551616 : value
  end

  def self.__arg__(value, default)
    value.nil? ? default : __int__(value)
  end

  # `do_checksum`: kind 0 is adler32, 1 is crc32.
  def self.__checksum__(kind, string, sum)
    if !sum.nil?
      sum = __ulong__(sum)
    elsif string.nil?
      sum = 0
    else
      sum = kind == 0 ? 1 : 0
    end
    if string.nil?
      # func(sum, Z_NULL, 0): the initial value, whatever `sum` was.
      return kind == 0 ? 1 : 0
    elsif IO === string
      until (chunk = string.read(8192)).nil?
        chunk = IO.__str__(chunk)
        sum = __zlib_checksum__(kind, sum & 0xffffffff, chunk)
      end
    else
      string = IO.__str__(string)
      sum = __zlib_checksum__(kind, sum & 0xffffffff, string)
    end
    sum
  end

  CRC_POLY__ = 0xedb88320
  private_constant :CRC_POLY__

  # crc32.c's `multmodp`: a(x) times b(x) modulo the CRC polynomial.
  def self.__multmodp__(a, b)
    m = 1 << 31
    p = 0
    loop do
      if a & m != 0
        p ^= b
        break if a & (m - 1) == 0
      end
      m >>= 1
      break if m == 0
      b = b & 1 != 0 ? (b >> 1) ^ CRC_POLY__ : b >> 1
    end
    p
  end

  # x^(2^n) modulo the polynomial, for n from 0 to 31.
  def self.__x2n_table__
    @__x2n_table__ ||= begin
      table = [1 << 30]
      31.times { table << __multmodp__(table.last, table.last) }
      table.freeze
    end
  end

  module_function

  def zlib_version
    "1.3.1".b
  end

  def adler32(string = nil, adler = nil)
    Zlib.__checksum__(0, string, adler)
  end

  def crc32(string = nil, crc = nil)
    Zlib.__checksum__(1, string, crc)
  end

  # adler32.c's `adler32_combine`.
  def adler32_combine(adler1, adler2, len2)
    adler1 = Zlib.__ulong__(adler1)
    adler2 = Zlib.__ulong__(adler2)
    len2 = Integer.__c_long__(len2)
    return 0xffffffff if len2 < 0
    base = 65521
    rem = len2 % base
    sum1 = adler1 & 0xffff
    sum2 = (rem * sum1) % base
    sum1 += (adler2 & 0xffff) + base - 1
    sum2 += ((adler1 >> 16) & 0xffff) + ((adler2 >> 16) & 0xffff) + base - rem
    sum1 -= base if sum1 >= base
    sum1 -= base if sum1 >= base
    sum2 -= base << 1 if sum2 >= base << 1
    sum2 -= base if sum2 >= base
    sum1 | (sum2 << 16)
  end

  # crc32.c's `crc32_combine`. A negative length is a C `z_off64_t` shifted
  # right forever there, which never ends; here the shifting stops after the
  # 64 bits a long has, where every further step would repeat.
  def crc32_combine(crc1, crc2, len2)
    crc1 = Zlib.__ulong__(crc1)
    crc2 = Zlib.__ulong__(crc2)
    n = Integer.__c_long__(len2)
    table = Zlib.__x2n_table__
    p = 1 << 31
    k = 3
    steps = 0
    while n != 0 && steps < 64
      p = Zlib.__multmodp__(table[k & 31], p) if n & 1 != 0
      n >>= 1
      k += 1
      steps += 1
    end
    Zlib.__multmodp__(p, crc1 & 0xffffffff) ^ (crc2 & 0xffffffff)
  end

  def crc_table
    table = []
    256.times do |n|
      c = n
      8.times { c = c & 1 != 0 ? (c >> 1) ^ 0xedb88320 : c >> 1 }
      table << c
    end
    table
  end

  # --- the stream ------------------------------------------------------------

  # `struct zstream` and the gzip file's `struct gzfile` around it: the
  # stream's state String, its input and output buffers, and its flags.
  class Stream__
    READY = 1 << 0
    IN_STREAM = 1 << 1
    FINISHED = 1 << 2
    CLOSING = 1 << 3
    GZFILE = 1 << 4
    REUSE_BUFFER = 1 << 5
    IN_PROGRESS = 1 << 6
    UNUSED = 1 << 7
    SYNC = UNUSED
    HEADER_FINISHED = UNUSED << 1
    FOOTER_FINISHED = UNUSED << 2
    MTIME_IS_SET = UNUSED << 3

    INITIAL_BUFSIZE = 1024
    STEP_MAX = 16384
    STEP_MIN = 2048

    attr_accessor :flags, :buf, :capa, :embedded, :avail_out, :input, :input_owned,
                  :state, :owner, :avail_in, :total_in_extra,
                  :io, :level, :os_code, :mtime, :orig_name, :comment, :crc,
                  :lineno, :ungetc, :enc, :enc2, :ecopts, :path, :writer
    attr_reader :deflating

    def initialize(deflating)
      @deflating = deflating
      @flags = 0
      @buf = nil
      @capa = 0
      @embedded = false
      @avail_out = 0
      @input = nil
      @input_owned = true
      @state = nil
      @owner = nil
      @avail_in = 0
      @total_in_extra = 0
    end

    # `gzfile_init`.
    def gzfile_init(writer)
      @flags |= GZFILE
      @writer = writer
      @io = nil
      @level = 0
      @mtime = 0
      @os_code = OS_CODE
      @orig_name = nil
      @comment = nil
      @crc = 0
      @lineno = 0
      @ungetc = 0
      @enc = Encoding.default_external
      @enc2 = nil
      @ecopts = nil
      @path = nil
      self
    end

    def initialize_copy(other)
      super
      @state = other.state&.dup
      @buf = other.buf&.dup
      @input = other.input&.dup
      @input_owned = true
    end

    def ready? = @flags & READY != 0
    def finished? = @flags & FINISHED != 0
    def gzfile? = @flags & GZFILE != 0
    def in_stream? = @flags & IN_STREAM != 0
    def reuse_buffer? = @flags & REUSE_BUFFER != 0
    def filled = @buf.nil? ? 0 : @buf.bytesize

    # `GZFILE_IS_FINISHED`.
    def gz_finished? = finished? && filled == 0

    def init(*args)
      made = __zlib_init__(*args)
      Zlib.__raise__(made, nil) if Integer === made
      @state = made
      @flags |= READY
    end

    def ctl(op, *args)
      __zlib_ctl__(op, @state, *args)
    end

    def info = ctl(6)
    def total_in = info[0] + @total_in_extra
    def total_out = info[1]
    def adler = info[2]
    def data_type = info[3]

    # --- the output buffer: what CRuby's String would hold, and its room ---

    # `rb_str_buf_new(capa)`: its capacity, and whether it is embedded in
    # one of the 40-, 80-, 160-, 320- or 640-byte slots a String can fill.
    def self.buf_capacity(capa)
      size = 25 + capa
      return [capa, false] if size > 640
      slot = 40
      slot *= 2 while slot < size
      [slot - 25, true]
    end

    # `rb_str_modify_expand(buf, expand)`.
    def modify_expand(expand)
      return if expand <= 0
      wanted = filled + expand
      if @embedded
        if @capa < wanted
          @capa = wanted
          @embedded = false
        end
      else
        @capa = wanted
      end
    end

    def new_buffer(bytes, room)
      @buf = bytes
      @capa, @embedded = Stream__.buf_capacity(room)
    end

    # `zstream_expand_buffer_into`.
    def expand_buffer_into(size)
      if @buf.nil?
        new_buffer("".b, size)
        @avail_out = size
      elsif @avail_out != size
        modify_expand(size)
        @avail_out = size
      end
    end

    # `zstream_expand_buffer_non_stream`.
    def expand_buffer_non_stream
      len = filled
      if @capa - len >= STEP_MAX
        @avail_out = STEP_MAX
      else
        inc = len / 2
        inc = STEP_MIN if inc < STEP_MIN
        modify_expand(inc)
        @avail_out = inc < STEP_MAX ? inc : STEP_MAX
      end
    end

    # `zstream_expand_buffer`: given a block, the full buffer is the block's.
    def expand_buffer(block)
      if @buf.nil?
        expand_buffer_into(INITIAL_BUFSIZE)
        return
      end
      if !gzfile? && block
        buf_filled = filled
        if buf_filled >= STEP_MAX
          chunk = @buf
          begin
            block.call(chunk)
          ensure
            if reuse_buffer?
              @buf.clear
            else
              @buf = nil
            end
            expand_buffer_into(STEP_MAX)
          end
        else
          expand_buffer_into(STEP_MAX - buf_filled)
        end
      else
        expand_buffer_non_stream
      end
    end

    # Bytes appended where the buffer's encoding, which a user's `buffer:`
    # may give it, is left as it is.
    def put(bytes)
      return if bytes.empty?
      if @buf.encoding == Encoding::BINARY
        @buf << bytes
      else
        encoding = @buf.encoding
        @buf.force_encoding(Encoding::BINARY)
        @buf << bytes
        @buf.force_encoding(encoding)
      end
    end

    # `zstream_append_buffer`.
    def append_buffer(src)
      src = src.b
      len = src.bytesize
      if @buf.nil?
        new_buffer(src, len)
        @avail_out = 0
        return
      end
      if @capa < filled + len
        modify_expand(len)
        @avail_out = 0
      else
        @avail_out = @avail_out >= len ? @avail_out - len : 0
      end
      put(src)
    end

    # `zstream_detach_buffer`.
    def detach_buffer(block)
      return nil if !finished? && !gzfile? && block
      dst = @buf.nil? ? "".b : @buf
      @buf = nil
      @avail_out = 0
      if !gzfile? && block
        block.call(dst)
        dst = nil
      end
      dst
    end

    # `zstream_shift_buffer`.
    def shift_buffer(len, dst)
      buflen = filled
      if buflen <= len
        return detach_buffer(nil) if dst.nil?
        encoding = dst.encoding
        dst.replace(@buf)
        dst.force_encoding(encoding)
        @buf = "".b
      else
        head = @buf.byteslice(0, len)
        if dst.nil?
          dst = head
        else
          encoding = dst.encoding
          dst.replace(head)
          dst.force_encoding(encoding)
        end
        @buf = @buf.byteslice(len, buflen - len)
      end
      room = @capa - filled
      room = STEP_MAX if room > STEP_MAX
      @avail_out = room
      dst
    end

    # `zstream_buffer_ungets`.
    def buffer_ungets(bytes)
      len = bytes.bytesize
      expand_buffer_into(len)
      @buf = bytes.b + @buf
      if @avail_out > 0
        len = @avail_out if len > @avail_out
        @avail_out -= len
      end
    end

    # --- the input buffer ---

    # `zstream_append_input`.
    def append_input(src)
      return if src.bytesize <= 0
      if @input.nil?
        @input = src.b
        @input_owned = true
      else
        @input = @input.b unless @input_owned
        @input_owned = true
        @input << src.b
      end
    end

    # `zstream_discard_input`.
    def discard_input(len)
      return if @input.nil?
      if @input.bytesize <= len
        @input = nil
      else
        @input = @input.byteslice(len, @input.bytesize - len)
      end
    end

    # `zstream_reset_input`: our own buffer is kept, emptied.
    def reset_input
      @input = @input_owned && !@input.nil? ? "".b : nil
    end

    # `zstream_passthrough_input`.
    def passthrough_input
      return if @input.nil?
      append_buffer(@input)
      @input = nil
    end

    # `zstream_detach_input`.
    def detach_input
      dst = @input.nil? ? "".b : @input
      @input = nil
      dst
    end

    # --- the stream's calls ---

    # `zstream_reset`.
    def reset
      err = ctl(0)[0]
      Zlib.__raise__(err, nil) if err != Z_OK__
      @flags = READY
      @buf = nil
      @avail_out = 0
      @total_in_extra = 0
      reset_input
    end

    def self.warning(message)
      warn(message, uplevel: 2) if $VERBOSE
    end

    # `zstream_end`.
    def end_
      unless ready?
        Stream__.warning("attempt to close uninitialized zstream; ignored.")
        return nil
      end
      if in_stream?
        Stream__.warning("attempt to close unfinished zstream; reset forced.")
        reset
      end
      reset_input
      err, msg = ctl(1)
      Zlib.__raise__(err, msg) if err != Z_OK__
      @flags = 0
      nil
    end

    # `zstream_run`: deflate or inflate `src` after what input is waiting,
    # with `flush`. `block` is the caller's, which gets the output as it
    # fills (not for a gzip file's stream).
    def run(src, flush, block = nil)
      raise InProgressError, "zlib stream is in progress" if @flags & IN_PROGRESS != 0
      @flags |= IN_PROGRESS
      block = nil if @flags & GZFILE != 0
      input = nil
      offset = 0
      left = true
      begin
        if @input.nil? || @input.empty?
          # Nothing waits: the run reads `src` itself, which zlib.c copies
          # into the input buffer first, and what it leaves is copied after.
          input = src
          @input = nil
        else
          append_input(src)
          input = detach_input
        end
        expand_buffer(block) if @avail_out == 0
        stream = block ? 1 : 0
        while true
          # `zstream_run_func`, its loop in `__zlib_run__`; a block given
          # is yielded the buffer here, between calls.
          while true
            err, consumed, out, msg, capa, avail, flags =
              __zlib_run__(@state, input, offset, flush, @buf.nil? ? 0 : @buf.bytesize, @capa, @avail_out, stream)
            offset += consumed
            unless out.empty?
              if @buf.encoding == Encoding::BINARY
                @buf << out
              else
                put(out)
              end
            end
            @capa = capa
            @avail_out = avail
            if flags != 0
              @flags = (@flags & ~IN_STREAM) | FINISHED if flags & 1 != 0
              @flags |= IN_STREAM if flags & 2 != 0
            end
            break if flags & 4 == 0
            expand_buffer(block)
          end
          @flags |= IN_STREAM if err == Z_BUF_ERROR__ && flush != FINISH && @avail_out > 0
          reset_input
          if err != Z_OK__ && err != Z_STREAM_END__
            rest_input(input, offset)
            if err == Z_NEED_DICT__ && @owner
              dicts = @owner.instance_variable_get(:@dictionaries)
              dict = dicts && dicts[adler]
              unless dict.nil?
                @owner.__set_dictionary__(dict)
                reset_input
                next
              end
            end
            left = false
            Zlib.__raise__(err, msg)
          end
          rest_input(input, offset)
          left = false
          return
        end
      ensure
        # Left part way, by a block that broke out or raised: what is left
        # of the input waits, and the stream's error, if any, is not raised.
        if left && !input.nil?
          reset_input
          rest_input(input, offset)
        end
        @flags &= ~IN_PROGRESS
      end
    end

    def rest_input(input, offset)
      @avail_in = input.bytesize - offset
      append_input(input.byteslice(offset, @avail_in)) if @avail_in > 0
    end

    # `zstream_sync`.
    def sync(src)
      unless @input.nil?
        err, consumed, msg = ctl(4, @input)
        @avail_in = @input.bytesize - consumed
        if err == Z_OK__
          discard_input(consumed)
          append_input(src)
          return true
        end
        reset_input
        Zlib.__raise__(err, msg) if err != Z_DATA_ERROR__
      end
      return false if src.bytesize <= 0
      err, consumed, msg = ctl(4, src)
      @avail_in = src.bytesize - consumed
      if err == Z_OK__
        append_input(src.byteslice(consumed, src.bytesize - consumed))
        return true
      end
      Zlib.__raise__(err, msg) if err != Z_DATA_ERROR__
      false
    end
  end
  private_constant :Stream__

  # --- ZStream, Deflate, Inflate ---------------------------------------------

  class ZStream
    def self.allocate
      raise TypeError, "allocator undefined for #{self}" if equal?(Zlib::ZStream)
      super
    end

    # `rb_undef_alloc_func`: there is no ZStream of its own, only a Deflate
    # or an Inflate, which have an `initialize` of their own.
    def initialize(*)
      raise TypeError, "allocator undefined for #{self.class}"
    end

    # The stream, ready or not (`TypedData_Get_Struct`).
    def __stream__
      @__z__ ||= Stream__.new(Deflate === self)
    end

    # `get_zstream`.
    def __z__
      z = __stream__
      raise Zlib::Error, "stream is not ready" unless z.ready?
      z
    end

    def initialize_copy(other)
      super
      @__z__ = Stream__.new(Deflate === self)
    end
    private :initialize_copy, :__stream__, :__z__

    def avail_out
      __stream__.avail_out
    end

    def avail_out=(size)
      __z__.expand_buffer_into(Zlib.__int__(size))
      size
    end

    def avail_in
      input = __stream__.input
      input.nil? ? 0 : input.bytesize
    end

    def total_in = __z__.total_in
    def total_out = __z__.total_out
    def data_type = __z__.data_type
    def adler = __z__.adler

    def finished?
      __z__.finished?
    end
    alias stream_end? finished?

    def closed?
      !__stream__.ready?
    end
    alias ended? closed?

    def close
      __z__.end_
      nil
    end
    alias end close

    def reset
      __z__.reset
      nil
    end

    def finish(&block)
      z = __z__
      z.run("".b, FINISH, block)
      z.detach_buffer(block)
    end

    def flush_next_in
      __stream__.detach_input
    end

    def flush_next_out(&block)
      __stream__.detach_buffer(block)
    end
  end

  class Deflate < ZStream
    def self.deflate(src, level = nil, &block)
      level = Zlib.__arg__(level, DEFAULT_COMPRESSION)
      src = IO.__str__(src)
      z = Stream__.new(true)
      z.init(level, 8, MAX_WBITS, DEF_MEM_LEVEL, DEFAULT_STRATEGY)
      begin
        z.run(src, FINISH, block)
        z.detach_buffer(block)
      ensure
        z.end_
      end
    end

    def initialize(level = nil, wbits = nil, memlevel = nil, strategy = nil)
      z = __stream__
      z.owner = self
      z.init(Zlib.__arg__(level, DEFAULT_COMPRESSION), 8, Zlib.__arg__(wbits, MAX_WBITS),
             Zlib.__arg__(memlevel, DEF_MEM_LEVEL), Zlib.__arg__(strategy, DEFAULT_STRATEGY))
      self
    end

    # `deflateCopy`, and the buffers with it.
    def initialize_copy(other)
      return self if other.equal?(self)
      theirs = other.__send__(:__z__)
      copy = theirs.dup
      copy.owner = self
      @__z__ = copy
      self
    end
    private :initialize_copy

    def __do_deflate__(z, src, flush, block)
      if src.nil?
        z.run("".b, FINISH, block)
        return
      end
      src = IO.__str__(src)
      z.run(src, flush, block) if flush != NO_FLUSH || src.bytesize > 0
    end
    private :__do_deflate__

    def deflate(src, flush = nil, &block)
      z = __z__
      __do_deflate__(z, src, Zlib.__arg__(flush, NO_FLUSH), block)
      z.detach_buffer(block)
    end

    def <<(src)
      __do_deflate__(__z__, src, NO_FLUSH, nil)
      self
    end

    def flush(flush = nil, &block)
      z = __z__
      flush = Zlib.__arg__(flush, SYNC_FLUSH)
      z.run("".b, flush, block) if flush != NO_FLUSH
      z.detach_buffer(block)
    end

    def params(level, strategy)
      z = __z__
      level = Zlib.__arg__(level, DEFAULT_COMPRESSION)
      strategy = Zlib.__arg__(strategy, DEFAULT_STRATEGY)
      err, out, msg = z.ctl(3, level, strategy, z.buf.nil? ? nil : z.avail_out)
      while err == -5
        Stream__.warning("deflateParams() returned Z_BUF_ERROR")
        unless z.buf.nil?
          z.put(out)
          z.avail_out -= out.bytesize
        end
        z.expand_buffer(nil)
        err, out, msg = z.ctl(3, level, strategy, z.avail_out)
      end
      Zlib.__raise__(err, msg) if err != 0
      z.put(out) unless z.buf.nil?
      z.avail_out -= out.bytesize unless z.buf.nil?
      nil
    end

    def set_dictionary(dictionary)
      z = __z__
      src = IO.__str__(dictionary)
      err, msg = z.ctl(2, src)
      Zlib.__raise__(err, msg) if err != 0
      dictionary
    end
  end

  class Inflate < ZStream
    def self.inflate(src, &block)
      src = IO.__str__(src)
      z = Stream__.new(false)
      z.init(MAX_WBITS)
      begin
        z.run(src, SYNC_FLUSH, block)
        z.run("".b, FINISH, block)
        z.detach_buffer(block)
      ensure
        z.end_
      end
    end

    def initialize(window_bits = nil)
      @dictionaries ||= {}
      z = __stream__
      z.owner = self
      z.init(Zlib.__arg__(window_bits, MAX_WBITS))
      self
    end

    def add_dictionary(dictionary)
      @dictionaries ||= {}
      @dictionaries[Zlib.adler32(dictionary)] = dictionary
      self
    end

    def __do_inflate__(z, src, block)
      if src.nil?
        z.run("".b, FINISH, block)
        return
      end
      src = IO.__str__(src)
      z.run(src, SYNC_FLUSH, block) if src.bytesize > 0 || z.avail_in > 0
    end
    private :__do_inflate__

    def inflate(src, buffer: nil, &block)
      z = __z__
      buffer = IO.__str__(buffer) unless buffer.nil?
      if !buffer.nil?
        unless z.reuse_buffer? && z.buf.equal?(buffer)
          raise FrozenError, "can't modify frozen String: #{buffer.inspect}" if buffer.frozen?
          len = buffer.bytesize
          buffer.clear
          z.flags |= Stream__::REUSE_BUFFER
          z.buf = buffer
          z.capa = len >= Stream__::STEP_MAX ? len : Stream__::STEP_MAX
          z.embedded = false
        end
      elsif z.reuse_buffer?
        z.flags &= ~Stream__::REUSE_BUFFER
        z.buf = nil
      end
      if z.finished?
        if src.nil?
          dst = z.detach_buffer(block)
        else
          src = IO.__str__(src)
          z.append_buffer(src)
          dst = z.reuse_buffer? ? buffer.clear : "".b
        end
      else
        __do_inflate__(z, src, block)
        dst = z.detach_buffer(block)
        z.passthrough_input if z.finished?
      end
      dst
    end

    def <<(src)
      z = __z__
      if z.finished?
        z.append_buffer(IO.__str__(src)) unless src.nil?
      else
        __do_inflate__(z, src, nil)
        z.passthrough_input if z.finished?
      end
      self
    end

    def sync(src)
      z = __z__
      z.sync(IO.__str__(src))
    end

    def sync_point?
      z = __z__
      answer, msg = z.ctl(5)
      return true if answer == 1
      Zlib.__raise__(answer, msg) if answer != 0
      false
    end

    def __set_dictionary__(dictionary)
      z = __z__
      src = IO.__str__(dictionary)
      err, msg = z.ctl(2, src)
      Zlib.__raise__(err, msg) if err != 0
      dictionary
    end

    def set_dictionary(dictionary)
      __set_dictionary__(dictionary)
    end
  end

  def self.deflate(src, level = nil, &block)
    Deflate.deflate(src, level, &block)
  end

  def self.inflate(src, &block)
    Inflate.inflate(src, &block)
  end

  # --- gzip --------------------------------------------------------------------

  class GzipFile
    class Error < Zlib::Error
      attr_reader :input

      def inspect
        str = super
        input = @input
        return str if input.nil?
        str.byteslice(0, str.bytesize - 1) + ", input=" + input.inspect + ">"
      end
    end

    class NoFooter < Error; end
    class CRCError < Error; end
    class LengthError < Error; end

    GZ_MAGIC1 = 0x1f
    GZ_MAGIC2 = 0x8b
    GZ_METHOD_DEFLATE = 8
    GZ_FLAG_MULTIPART = 0x2
    GZ_FLAG_EXTRA = 0x4
    GZ_FLAG_ORIG_NAME = 0x8
    GZ_FLAG_COMMENT = 0x10
    GZ_FLAG_ENCRYPT = 0x20
    GZ_FLAG_UNKNOWN_MASK = 0xc0
    GZ_EXTRAFLAG_FAST = 0x4
    GZ_EXTRAFLAG_SLOW = 0x2
    READ_SIZE = 2048
    private_constant :GZ_MAGIC1, :GZ_MAGIC2, :GZ_METHOD_DEFLATE, :GZ_FLAG_MULTIPART,
                     :GZ_FLAG_EXTRA, :GZ_FLAG_ORIG_NAME, :GZ_FLAG_COMMENT, :GZ_FLAG_ENCRYPT,
                     :GZ_FLAG_UNKNOWN_MASK, :GZ_EXTRAFLAG_FAST, :GZ_EXTRAFLAG_SLOW, :READ_SIZE

    class << self
      def allocate
        raise TypeError, "allocator undefined for #{self}" if equal?(Zlib::GzipFile)
        super
      end

      # `gzfile_wrap`.
      def wrap(*args, **opts, &block)
        __wrap__(args, opts, false, &block)
      end

      def __wrap__(args, opts, close_io_on_error, &block)
        if close_io_on_error
          begin
            obj = new(*args, **opts)
          rescue Exception
            args[0].close
            raise
          end
        else
          obj = new(*args, **opts)
        end
        return obj unless block
        begin
          yield obj
        ensure
          z = obj.__send__(:__stream__)
          obj.__send__(:__close__, z, true) if z.ready?
        end
      end

      # `gzfile_s_open`.
      def __open__(args, opts, mode, &block)
        raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if args.empty?
        io = File.open(args[0], mode)
        __wrap__([io, *args[1..]], opts, true, &block)
      end
      private :__wrap__, :__open__
    end

    def initialize(*)
      raise TypeError, "allocator undefined for #{self.class}"
    end

    def __stream__
      @__z__ ||= Stream__.new(GzipWriter === self).gzfile_init(GzipWriter === self)
    end

    # `get_gzfile`.
    def __z__
      z = __stream__
      raise GzipFile::Error, "closed gzip stream" unless z.ready?
      z
    end

    def initialize_copy(other)
      super
      @__z__ = nil
    end
    private :initialize_copy, :__stream__, :__z__

    # `gzfile_close`.
    def __close__(z, closeflag)
      io = z.io
      __end__(z)
      z.io = nil
      z.orig_name = nil
      z.comment = nil
      io.close if closeflag && io.respond_to?(:close)
    end
    private :__close__

    def to_io = __z__.io
    def crc = __z__.crc
    def mtime = Time.at(__z__.mtime)
    def level = __z__.level
    def os_code = __z__.os_code

    def orig_name
      name = __z__.orig_name
      name.nil? ? nil : name.dup
    end

    def comment
      text = __z__.comment
      text.nil? ? nil : text.dup
    end

    def close
      z = __stream__
      return nil unless z.ready?
      io = z.io
      __close__(z, true)
      io
    end

    def finish
      z = __z__
      io = z.io
      __close__(z, false)
      io
    end

    def closed?
      __stream__.io.nil?
    end

    def sync
      __z__.flags & Stream__::SYNC != 0
    end

    def sync=(mode)
      z = __z__
      if mode
        z.flags |= Stream__::SYNC
      else
        z.flags &= ~Stream__::SYNC
      end
      mode
    end

    # `rb_gzfile_ecopts`, over `rb_io_extract_encoding_option`.
    def __ecopts__(z, opts)
      return if opts.nil? || opts.empty?
      ext = int = nil
      given = false
      if opts.key?(:encoding) && !opts[:encoding].nil?
        given = true
        spec = opts[:encoding]
        if Encoding === spec
          ext = spec
        else
          names = String(spec).split(":", 2)
          ext = Encoding.find(names[0].sub(/\ABOM\|/i, ""))
          int = names[1] && Encoding.find(names[1])
        end
      end
      if opts.key?(:external_encoding) && !opts[:external_encoding].nil?
        given = true
        ext = Encoding.find(opts[:external_encoding])
      end
      if opts.key?(:internal_encoding) && !opts[:internal_encoding].nil?
        given = true
        int = Encoding.find(opts[:internal_encoding])
      end
      return unless given
      ext ||= Encoding.default_external
      int = Encoding.default_internal if int.nil? && ext != Encoding::BINARY
      if int.nil? || int == ext
        z.enc = ext
        z.enc2 = nil
      else
        z.enc = int
        z.enc2 = ext
        z.ecopts = opts.reject { |k, _| %i[encoding external_encoding internal_encoding].include?(k) }
      end
    end
    private :__ecopts__

    def __path__(z)
      return unless z.io.respond_to?(:path)
      begin
        path = z.io.path
      rescue IOError
        return
      end
      z.path = path
      define_singleton_method(:path) { path }
    end
    private :__path__
  end

  class GzipWriter < GzipFile
    def self.open(*args, **opts, &block)
      __open__(args, opts, "wb", &block)
    end

    def initialize(io, *rest, **opts)
      rest << opts unless opts.empty?
      opt = nil
      if rest.size >= 1 && (Hash === rest.last || rest.last.respond_to?(:to_hash))
        opt = rest.pop
        opt = opt.to_hash unless Hash === opt
      end
      if rest.size > 2
        raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 1..3)"
      end
      level, strategy = rest
      z = __stream__
      z.level = Zlib.__arg__(level, DEFAULT_COMPRESSION)
      z.init(z.level, 8, -MAX_WBITS, DEF_MEM_LEVEL, Zlib.__arg__(strategy, DEFAULT_STRATEGY))
      z.io = io
      __ecopts__(z, opt)
      __path__(z)
      self
    end

    def mtime=(mtime)
      z = __z__
      raise GzipFile::Error, "header is already written" if z.flags & Stream__::HEADER_FINISHED != 0
      value = Integer(mtime)
      value = IO.__c_uint__(value)
      z.mtime = value & 0xffffffff
      z.flags |= Stream__::MTIME_IS_SET
      mtime
    end

    def orig_name=(str)
      z = __z__
      raise GzipFile::Error, "header is already written" if z.flags & Stream__::HEADER_FINISHED != 0
      s = IO.__str__(str).dup
      nul = s.byteindex("\0")
      s = s.byteslice(0, nul) unless nul.nil?
      z.orig_name = s
      str
    end

    def comment=(str)
      z = __z__
      raise GzipFile::Error, "header is already written" if z.flags & Stream__::HEADER_FINISHED != 0
      s = IO.__str__(str).dup
      nul = s.byteindex("\0")
      s = s.byteslice(0, nul) unless nul.nil?
      z.comment = s
      str
    end

    def pos = __z__.total_in
    alias tell pos

    # `gzfile_make_header`.
    def __make_header__(z)
      flags = 0
      flags |= GZ_FLAG_ORIG_NAME unless z.orig_name.nil?
      flags |= GZ_FLAG_COMMENT unless z.comment.nil?
      z.mtime = Time.now.to_i & 0xffffffff if z.flags & Stream__::MTIME_IS_SET == 0
      extraflags = 0
      if z.level == BEST_SPEED
        extraflags |= GZ_EXTRAFLAG_FAST
      elsif z.level == BEST_COMPRESSION
        extraflags |= GZ_EXTRAFLAG_SLOW
      end
      head = [GZ_MAGIC1, GZ_MAGIC2, GZ_METHOD_DEFLATE, flags, z.mtime, extraflags, z.os_code & 0xff]
      z.append_buffer(head.pack("CCCCVCC"))
      unless z.orig_name.nil?
        z.append_buffer(z.orig_name)
        z.append_buffer("\0")
      end
      unless z.comment.nil?
        z.append_buffer(z.comment)
        z.append_buffer("\0")
      end
      z.flags |= Stream__::HEADER_FINISHED
    end
    private :__make_header__

    # `gzfile_make_footer`.
    def __make_footer__(z)
      z.append_buffer([z.crc & 0xffffffff, z.total_in & 0xffffffff].pack("VV"))
      z.flags |= Stream__::FOOTER_FINISHED
    end
    private :__make_footer__

    # `gzfile_write_raw`.
    def __write_raw__(z)
      return unless z.filled > 0
      str = z.detach_buffer(nil)
      z.io.write(str)
      z.io.flush if z.flags & Stream__::SYNC != 0 && z.io.respond_to?(:flush)
    end
    private :__write_raw__

    # `gzfile_write`.
    def __write__(z, str)
      __make_header__(z) if z.flags & Stream__::HEADER_FINISHED == 0
      sync = z.flags & Stream__::SYNC != 0
      if str.bytesize > 0 || sync
        z.crc = __zlib_checksum__(1, z.crc, str)
        z.run(str, sync ? SYNC_FLUSH : NO_FLUSH)
      end
      __write_raw__(z)
    end
    private :__write__

    # `gzfile_writer_end`.
    def __end__(z)
      return if z.flags & Stream__::CLOSING != 0
      z.flags |= Stream__::CLOSING
      begin
        __make_header__(z) if z.flags & Stream__::HEADER_FINISHED == 0
        z.run("".b, FINISH)
        __make_footer__(z)
        __write_raw__(z)
      ensure
        z.end_
      end
    end
    private :__end__

    def flush(flush = nil)
      z = __z__
      flush = Zlib.__arg__(flush, SYNC_FLUSH)
      z.run("".b, flush) if flush != NO_FLUSH
      __write_raw__(z)
      z.io.flush if z.io.respond_to?(:flush)
      self
    end

    def write(*strs)
      z = __z__
      total = 0
      strs.each do |str|
        str = str.to_s unless String === str
        str = str.encode(z.enc2) if z.enc2 && z.enc2 != Encoding::BINARY
        __write__(z, str)
        total += str.bytesize
      end
      total
    end

    def putc(ch)
      z = __z__
      c = if String === ch && ch.bytesize >= 1
        ch.byteslice(0, 1)
      else
        (Zlib.__int__(ch) & 0xff).chr
      end
      __write__(z, c)
      ch
    end

    def <<(object)
      write(object)
      self
    end

    def print(*objects)
      objects = [$_] if objects.empty?
      objects.each_with_index do |object, index|
        write($,) if index > 0 && !$,.nil?
        write(object.nil? ? "" : object)
      end
      write($\) unless $\.nil?
      nil
    end

    def printf(format = nil, *args)
      return nil if format.nil?
      write(Kernel.format(format, *args))
      nil
    end

    def puts(*objects)
      if objects.empty?
        write("\n")
        return nil
      end
      __puts__(objects, [])
      nil
    end

    def __puts__(objects, seen)
      objects.each do |object|
        array = nil
        if Array === object
          array = object
        elsif !(String === object) && object.respond_to?(:to_ary)
          converted = object.to_ary
          array = converted if Array === converted
        end
        if array
          if seen.any? { |s| s.equal?(array) }
            write("[...]")
            write("\n")
            next
          end
          __puts__(array, seen + [array])
          next
        end
        line = String === object ? object : object.to_s
        line = Kernel.instance_method(:to_s).bind_call(object) unless String === line
        write(line)
        write("\n") unless line.end_with?("\n")
      end
    end
    private :__puts__
  end

  class GzipReader < GzipFile
    include Enumerable

    def self.open(*args, **opts, &block)
      __open__(args, opts, "rb", &block)
    end

    # `rb_gzreader_s_zcat`.
    def self.zcat(io, opts = nil, &block)
      args = opts.nil? ? [io] : [io, opts]
      buf = nil
      loop do
        obj = new(*args)
        if block
          obj.each(&block)
        else
          buf ||= "".b
          buf << obj.__send__(:__read_all__, obj.__send__(:__z__), nil).b
        end
        obj.read
        pos = io.pos
        unused = obj.unused
        obj.finish
        unless unused.nil?
          pos -= unused.length
          io.pos = pos
        end
        break unless pos < io.size
      end
      block ? nil : buf
    end

    def initialize(io, **opts)
      z = __stream__
      z.init(-MAX_WBITS)
      z.io = io
      __read_header__(z, nil)
      __ecopts__(z, opts)
      __path__(z)
      self
    end

    def lineno = __z__.lineno

    def lineno=(lineno)
      __z__.lineno = Zlib.__int__(lineno)
      lineno
    end

    # `gzfile_read_raw`: readpartial, or read where there is none.
    def __read_raw__(z, outbuf)
      args = outbuf.nil? ? [READ_SIZE] : [READ_SIZE, outbuf]
      io = z.io
      raise GzipFile::Error, "unexpected end of file" if io.equal?(GUNZIP__)
      begin
        str = io.readpartial(*args)
        raise TypeError, "wrong argument type #{str.class} (expected String)" unless String === str
        str
      rescue EOFError
        nil
      rescue NoMethodError
        str = io.read(*args)
        if !str.nil? && !(String === str)
          raise TypeError, "wrong argument type #{str.class} (expected String)"
        end
        str
      end
    end
    private :__read_raw__

    # `gzfile_read_raw_ensure`.
    def __read_raw_ensure__(z, size, outbuf)
      if z.io.equal?(GUNZIP__)
        raise GzipFile::Error, "unexpected end of string" if z.input.nil? || z.input.bytesize < size
      end
      while z.input.nil? || z.input.bytesize < size
        str = __read_raw__(z, outbuf)
        return false if str.nil?
        z.append_input(str)
      end
      true
    end
    private :__read_raw_ensure__

    # `gzfile_read_raw_until_zero`: where the next NUL is in the input.
    def __read_raw_until_zero__(z, offset)
      loop do
        at = z.input.byteindex("\0", offset)
        return at unless at.nil?
        str = __read_raw__(z, nil)
        raise GzipFile::Error, "unexpected end of file" if str.nil?
        offset = z.input.bytesize
        z.append_input(str)
      end
    end
    private :__read_raw_until_zero__

    # `gzfile_raise`: the error, with the input that was waiting.
    def __gz_raise__(z, klass, message)
      error = klass.new(message)
      error.instance_variable_set(:@input, z.input.dup) unless z.input.nil?
      raise error
    end
    private :__gz_raise__

    # `gzfile_read_header`.
    def __read_header__(z, outbuf)
      __gz_raise__(z, GzipFile::Error, "not in gzip format") unless __read_raw_ensure__(z, 10, outbuf)
      head = z.input
      if head.getbyte(0) != GZ_MAGIC1 || head.getbyte(1) != GZ_MAGIC2
        __gz_raise__(z, GzipFile::Error, "not in gzip format")
      end
      if head.getbyte(2) != GZ_METHOD_DEFLATE
        raise GzipFile::Error, "unsupported compression method #{head.getbyte(2)}"
      end
      flags = head.getbyte(3)
      if flags & GZ_FLAG_MULTIPART != 0
        raise GzipFile::Error, "multi-part gzip file is not supported"
      elsif flags & GZ_FLAG_ENCRYPT != 0
        raise GzipFile::Error, "encrypted gzip file is not supported"
      elsif flags & GZ_FLAG_UNKNOWN_MASK != 0
        # A C char, printed with %02x: one with the high bit set is negative.
        shown = flags >= 0x80 ? flags + 0xffffff00 : flags
        raise GzipFile::Error, format("unknown flags 0x%02x", shown)
      end
      xfl = head.getbyte(8)
      z.level = if xfl & GZ_EXTRAFLAG_FAST != 0
        BEST_SPEED
      elsif xfl & GZ_EXTRAFLAG_SLOW != 0
        BEST_COMPRESSION
      else
        DEFAULT_COMPRESSION
      end
      z.mtime = head.byteslice(4, 4).unpack1("V")
      z.os_code = head.getbyte(9)
      z.discard_input(10)
      if flags & GZ_FLAG_EXTRA != 0
        raise GzipFile::Error, "unexpected end of file" unless __read_raw_ensure__(z, 2, outbuf)
        len = z.input.byteslice(0, 2).unpack1("v")
        raise GzipFile::Error, "unexpected end of file" unless __read_raw_ensure__(z, 2 + len, outbuf)
        z.discard_input(2 + len)
      end
      if flags & GZ_FLAG_ORIG_NAME != 0
        raise GzipFile::Error, "unexpected end of file" unless __read_raw_ensure__(z, 1, outbuf)
        len = __read_raw_until_zero__(z, 0)
        z.orig_name = z.input.byteslice(0, len)
        z.discard_input(len + 1)
      end
      if flags & GZ_FLAG_COMMENT != 0
        raise GzipFile::Error, "unexpected end of file" unless __read_raw_ensure__(z, 1, outbuf)
        len = __read_raw_until_zero__(z, 0)
        z.comment = z.input.byteslice(0, len)
        z.discard_input(len + 1)
      end
      z.run("".b, SYNC_FLUSH) if !z.input.nil? && z.input.bytesize > 0
    end
    private :__read_header__

    # `gzfile_check_footer`.
    def __check_footer__(z, outbuf)
      z.flags |= Stream__::FOOTER_FINISHED
      __gz_raise__(z, NoFooter, "footer is not found") unless __read_raw_ensure__(z, 8, outbuf)
      crc, length = z.input.byteslice(0, 8).unpack("VV")
      z.total_in_extra += 8
      z.discard_input(8)
      raise CRCError, "invalid compressed data -- crc error" if z.crc != crc
      raise LengthError, "invalid compressed data -- length error" if z.total_out & 0xffffffff != length
    end
    private :__check_footer__

    # `gzfile_read_more`.
    def __read_more__(z, outbuf)
      until z.finished?
        str = __read_raw__(z, outbuf)
        if str.nil?
          raise GzipFile::Error, "unexpected end of file" unless z.finished?
          break
        end
        z.run(str, SYNC_FLUSH) if str.bytesize > 0
        break if z.filled > 0
      end
      z.filled
    end
    private :__read_more__

    # `gzfile_calc_crc`.
    def __calc_crc__(z, str)
      if str.bytesize <= z.ungetc
        z.ungetc -= str.bytesize
      else
        z.crc = __zlib_checksum__(1, z.crc, z.ungetc == 0 ? str : str.byteslice(z.ungetc, str.bytesize - z.ungetc))
        z.ungetc = 0
      end
    end
    private :__calc_crc__

    # `gzfile_newstr`.
    def __newstr__(z, str)
      if z.enc2.nil?
        str.force_encoding(z.enc)
        return str
      end
      str.force_encoding(z.enc2)
      begin
        opts = z.ecopts || {}
        str.encode(z.enc, **opts)
      rescue EncodingError
        str
      end
    end
    private :__newstr__

    # `gzfile_fill`.
    def __fill__(z, len, outbuf)
      raise ArgumentError, "negative length #{len} given" if len < 0
      return 0 if len == 0
      __read_more__(z, outbuf) while !z.finished? && z.filled < len
      if z.gz_finished?
        __check_footer__(z, outbuf) if z.flags & Stream__::FOOTER_FINISHED == 0
        return -1
      end
      len < z.filled ? len : z.filled
    end
    private :__fill__

    # `gzfile_read`.
    def __read__(z, len, outbuf)
      len = __fill__(z, len, outbuf)
      if len < 0
        outbuf&.clear
        return nil
      end
      if len == 0
        return "".b if outbuf.nil?
        outbuf.clear
        return outbuf
      end
      dst = z.shift_buffer(len, outbuf)
      __calc_crc__(z, dst) unless dst.nil?
      dst
    end
    private :__read__

    # `gzfile_read_all`.
    def __read_all__(z, dst)
      __read_more__(z, dst) until z.finished?
      if z.gz_finished?
        __check_footer__(z, dst) if z.flags & Stream__::FOOTER_FINISHED == 0
        unless dst.nil?
          dst.clear
          return dst
        end
        return "".b
      end
      dst = z.detach_buffer(nil)
      return dst if dst.nil?
      __calc_crc__(z, dst)
      __newstr__(z, dst)
    end
    private :__read_all__

    # `gzfile_reader_end`.
    def __end__(z)
      return if z.flags & Stream__::CLOSING != 0
      z.flags |= Stream__::CLOSING
      begin
        __check_footer__(z, nil) if z.gz_finished? && z.flags & Stream__::FOOTER_FINISHED == 0
      ensure
        z.end_
      end
    end
    private :__end__

    def rewind
      z = __z__
      n = z.total_in
      n += z.input.bytesize unless z.input.nil?
      z.io.seek(-n, IO::SEEK_CUR)
      z.reset
      z.flags |= Stream__::GZFILE
      z.crc = 0
      z.lineno = 0
      z.ungetc = 0
      0
    end

    def unused
      z = __stream__
      return nil unless z.ready?
      return nil unless z.gz_finished?
      __check_footer__(z, nil) if z.flags & Stream__::FOOTER_FINISHED == 0
      return nil if z.input.nil?
      z.input.dup
    end

    def read(len = nil, outbuf = nil)
      z = __z__
      return __read_all__(z, outbuf) if len.nil?
      len = Zlib.__int__(len)
      raise ArgumentError, "negative length #{len} given" if len < 0
      __read__(z, len, outbuf)
    end

    def readpartial(len, outbuf = nil)
      z = __z__
      len = Zlib.__int__(len)
      raise ArgumentError, "negative length #{len} given" if len < 0
      if !outbuf.nil? && !(String === outbuf)
        raise TypeError, "wrong argument type #{outbuf.class} (expected String)"
      end
      if len == 0
        return "".b if outbuf.nil?
        outbuf.clear
        return outbuf
      end
      __read_more__(z, outbuf) while !z.finished? && z.filled == 0
      if z.gz_finished?
        __check_footer__(z, outbuf) if z.flags & Stream__::FOOTER_FINISHED == 0
        outbuf&.clear
        raise EOFError, "end of file reached"
      end
      dst = z.shift_buffer(len, outbuf)
      __calc_crc__(z, dst)
      dst
    end

    def __mbmaxlen__(encoding)
      case encoding
      when Encoding::UTF_8, Encoding::UTF_16LE, Encoding::UTF_16BE, Encoding::UTF_32LE,
           Encoding::UTF_32BE, Encoding::GB18030
        4
      when Encoding::BINARY, Encoding::US_ASCII
        1
      else
        encoding.ascii_compatible? && "é".encode(encoding).bytesize == 1 ? 1 : 4 rescue 4
      end
    end
    private :__mbmaxlen__

    # `gzfile_getc`.
    def __getc__(z)
      len = __mbmaxlen__(z.enc)
      __read_more__(z, nil) while !z.finished? && z.filled < len
      if z.gz_finished?
        __check_footer__(z, nil) if z.flags & Stream__::FOOTER_FINISHED == 0
        return nil
      end
      head = z.buf.byteslice(0, len).force_encoding(z.enc)
      len = head.__char_len_at__(0)
      dst = __read__(z, len, nil)
      return dst if dst.nil?
      __newstr__(z, dst)
    end
    private :__getc__

    def getc = __getc__(__z__)

    def readchar
      c = getc
      raise EOFError, "end of file reached" if c.nil?
      c
    end

    def getbyte
      dst = __read__(__z__, 1, nil)
      dst.nil? ? nil : dst.getbyte(0)
    end

    def readbyte
      b = getbyte
      raise EOFError, "end of file reached" if b.nil?
      b
    end

    def each_char
      return enum_for(:each_char) unless block_given?
      while (c = getc)
        yield c
      end
      nil
    end

    def each_byte
      return enum_for(:each_byte) unless block_given?
      while (b = getbyte)
        yield b
      end
      nil
    end

    def ungetc(s)
      return ungetbyte(s) if Integer === s
      z = __z__
      s = IO.__str__(s)
      s = s.encode(z.enc2) if z.enc2 && z.enc2 != Encoding::BINARY
      z.buffer_ungets(s)
      z.ungetc += s.bytesize
      nil
    end

    def ungetbyte(ch)
      z = __z__
      c = if String === ch && ch.bytesize >= 1
        ch.byteslice(0, 1)
      else
        (Zlib.__int__(ch) & 0xff).chr
      end
      z.buffer_ungets(c)
      z.ungetc += 1
      nil
    end

    def eof
      z = __z__
      __read_more__(z, nil) while !z.finished? && z.filled == 0
      z.gz_finished?
    end
    alias eof? eof

    def pos
      z = __z__
      z.total_out - z.filled
    end
    alias tell pos

    def external_encoding = __z__.enc

    # `gzreader_skip_linebreaks`.
    def __skip_linebreaks__(z)
      while z.filled == 0
        return if z.gz_finished?
        __read_more__(z, nil)
      end
      n = 0
      loop do
        if z.buf.getbyte(n) != 0x0a
          break
        end
        n += 1
        if n >= z.filled
          str = z.detach_buffer(nil)
          __calc_crc__(z, str)
          while z.filled == 0
            return if z.gz_finished?
            __read_more__(z, nil)
          end
          n = 0
        end
      end
      str = z.shift_buffer(n, nil)
      __calc_crc__(z, str)
    end
    private :__skip_linebreaks__

    # `gzreader_charboundary`: `n`, moved past the character it cuts.
    def __charboundary__(z, n)
      s = z.buf.byteslice(0, z.filled).force_encoding(z.enc)
      offsets = s.__char_offsets__
      head = offsets.select { |o| o <= n - 1 }.last || 0
      if head < n
        rest = s.byteslice(head, s.bytesize - head)
        if rest.__cut_short__
          need = __mbmaxlen__(z.enc) - rest.bytesize
          need = 1 if need < 1
          l = __fill__(z, n + need, nil)
          return l if l > 0
        else
          len = rest.__char_len_at__(0)
          return head + len
        end
      end
      n
    end
    private :__charboundary__

    # `gzreader_gets`.
    def __gets__(args)
      z = __z__
      limit = -1
      maxlen = __mbmaxlen__(z.enc)
      if args.empty?
        rs = $/
      else
        if args.size > 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
        end
        rs, lim = args
        if !lim.nil?
          rs = IO.__str__(rs) unless rs.nil?
        elsif !rs.nil?
          if String === rs || rs.respond_to?(:to_str)
            rs = IO.__str__(rs)
          else
            lim = rs
            rs = $/
          end
        end
        unless lim.nil?
          limit = Integer.__c_long__(lim)
          return "".b if limit == 0
        end
      end
      if rs.nil?
        if limit < 0
          dst = __read_all__(z, nil)
          return nil if dst.bytesize == 0
        elsif (n = __fill__(z, limit, nil)) <= 0
          return nil
        else
          n = if maxlen > 1 && n >= limit && !z.gz_finished?
            __charboundary__(z, n)
          else
            limit
          end
          dst = z.shift_buffer(n, nil)
          return dst if dst.nil?
          __calc_crc__(z, dst)
          dst = __newstr__(z, dst)
        end
        z.lineno += 1
        return dst
      end
      if rs.bytesize == 0
        rsptr = "\n\n".b
        rspara = true
      else
        rsptr = rs.b
        rspara = false
      end
      rslen = rsptr.bytesize
      __skip_linebreaks__(z) if rspara
      while z.filled < rslen
        if z.finished?
          z.lineno += 1 if z.filled > 0
          return __read__(z, rslen, nil)
        end
        __read_more__(z, nil)
      end
      first = rsptr.byteslice(0, 1)
      p = 0
      n = rslen
      loop do
        if n > z.filled
          break if z.finished?
          __read_more__(z, nil)
          p = n - rslen
        end
        filled = z.filled
        filled = limit if limit > 0 && filled >= limit
        # memchr(p, rsptr[0], filled - n + 1)
        count = filled - n + 1
        res = count > 0 ? z.buf.__byte_index__(first, p) : nil
        res = nil if !res.nil? && res >= p + count
        if res.nil?
          n = filled
          break if limit > 0 && filled >= limit
          n += 1
        else
          n += res - p
          p = res
          break if rslen == 1 || z.buf.byteslice(p, rslen) == rsptr
          p += 1
          n += 1
        end
      end
      if maxlen > 1 && n == limit && (z.filled > n || !z.finished?)
        n = __charboundary__(z, n)
      end
      z.lineno += 1
      dst = __read__(z, n, nil)
      return dst if dst.nil?
      __skip_linebreaks__(z) if rspara
      __newstr__(z, dst)
    end
    private :__gets__

    def gets(*args)
      dst = __gets__(args)
      $_ = dst unless dst.nil?
      dst
    end

    def readline(*args)
      dst = gets(*args)
      raise EOFError, "end of file reached" if dst.nil?
      dst
    end

    def each(*args)
      return enum_for(:each, *args) unless block_given?
      while (str = __gets__(args))
        yield str
      end
      self
    end
    alias each_line each

    def readlines(*args)
      lines = []
      while (str = __gets__(args))
        lines << str
      end
      lines
    end
  end

  # The io of a gzip stream `Zlib.gunzip` reads from a String (`Qundef`).
  GUNZIP__ = Object.new.freeze
  private_constant :GUNZIP__

  def self.gzip(src, level: nil, strategy: nil)
    src = IO.__str__(src)
    z = Stream__.new(true).gzfile_init(true)
    z.level = __arg__(level, DEFAULT_COMPRESSION)
    z.init(z.level, 8, -MAX_WBITS, DEF_MEM_LEVEL, __arg__(strategy, DEFAULT_STRATEGY))
    writer = GzipWriter.allocate
    begin
      writer.__send__(:__make_header__, z)
      if src.bytesize > 0
        z.crc = __zlib_checksum__(1, z.crc, src)
        z.run(src, NO_FLUSH)
      end
      # `gzfile_close` over `zlib_gzip_end`.
      z.flags |= Stream__::CLOSING
      z.run("".b, FINISH)
      writer.__send__(:__make_footer__, z)
      z.end_
      z.io = nil
      z.detach_buffer(nil)
    ensure
      begin
        if z.ready?
          z.flags |= Stream__::CLOSING
          z.run("".b, FINISH)
          writer.__send__(:__make_footer__, z)
          z.end_
        end
      rescue StandardError
        nil
      end
    end
  end

  def self.gunzip(src)
    src = IO.__str__(src)
    z = Stream__.new(false).gzfile_init(false)
    z.init(-MAX_WBITS)
    z.io = GUNZIP__
    z.input = src
    z.input_owned = false
    reader = GzipReader.allocate
    begin
      reader.__send__(:__read_header__, z, nil)
      dst = z.detach_buffer(nil)
      reader.__send__(:__calc_crc__, z, dst)
      raise GzipFile::Error, "unexpected end of file" unless z.finished?
      raise GzipFile::NoFooter, "footer is not found" if z.input.nil?
      reader.__send__(:__check_footer__, z, nil)
      dst
    ensure
      begin
        if z.ready?
          z.flags |= Stream__::CLOSING
          z.end_
        end
      rescue StandardError
        nil
      end
    end
  end
end
