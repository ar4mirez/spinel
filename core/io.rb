# `IO` is a descriptor (#41): opened by something else and handed over as an
# Integer, or by `File`, `IO.sysopen` and `IO.pipe`. Reads are buffered,
# writes go straight out, and every system call is one primitive that
# answers an errno on failure. The rest, which is most of it, is Ruby.
#
# The read buffer is `@rbuf`, bytes the descriptor has moved past and the
# reader has not: `pos` subtracts it, a relative seek adjusts for it, and a
# write after a read seeks back over it first.
#
# ponytail: no write buffer, so `sync` is only recorded and `flush` has
# nothing to do; and conversion between an external and an internal encoding
# is done a whole read or a whole line at a time, by `String#encode`. A
# stream decoder is the upgrade, when a spec needs a line split across a
# conversion.

class IO
end

# `File::Constants` is here rather than in `core/file.rb` because `IO`
# includes it, and the standard streams below need its open flags.
class File < IO
  # The open and lock flags are the platform's; the `fnmatch` flags are
  # Ruby's own numbering.
  module Constants
    __fs_constants__.each { |name, value| const_set(name, value) }
    BINARY = 0
    SHARE_DELETE = 0
    FNM_SYSCASE = 0
    FNM_SHORTNAME = 0
    FNM_NOESCAPE = 1
    FNM_PATHNAME = 2
    FNM_DOTMATCH = 4
    FNM_CASEFOLD = 8
    FNM_EXTGLOB = 16
    NULL = "/dev/null"
  end
end

class IO
  include Enumerable
  include File::Constants

  SEEK_SET = 0
  SEEK_CUR = 1
  SEEK_END = 2
  SEEK_DATA = 3
  SEEK_HOLE = 4

  READABLE = 1
  PRIORITY = 2
  WRITABLE = 4

  module WaitReadable; end
  module WaitWritable; end

  class EAGAINWaitReadable < Errno::EAGAIN
    include IO::WaitReadable
  end

  class EAGAINWaitWritable < Errno::EAGAIN
    include IO::WaitWritable
  end

  EWOULDBLOCKWaitReadable = EAGAINWaitReadable
  EWOULDBLOCKWaitWritable = EAGAINWaitWritable

  class EINPROGRESSWaitReadable < Errno::EINPROGRESS
    include IO::WaitReadable
  end

  class EINPROGRESSWaitWritable < Errno::EINPROGRESS
    include IO::WaitWritable
  end

  class TimeoutError < IOError
  end

  def self.__poll__(name)
    (@__poll__ ||= __poll_constants__.to_h)[name]
  end

  # `poll(2)` over `[io, events]` pairs: the events each is ready for, or
  # nil when the timeout passed first. A reader with bytes buffered is ready
  # without asking.
  def self.__wait__(pairs, timeout, descriptor_only = false)
    unless timeout.nil?
      timeout = Float === timeout || Integer === timeout ? timeout : Float(timeout)
      raise ArgumentError, "time interval must not be negative" if timeout < 0
    end
    buffered = pairs.map do |io, events|
      !descriptor_only && events & IO::READABLE != 0 && io.__buffered__? ? IO::READABLE : 0
    end
    return buffered if buffered.any? { |b| b != 0 }
    polls = pairs.map do |io, events|
      bits = 0
      bits |= __poll__("POLLIN") if events & IO::READABLE != 0
      bits |= __poll__("POLLOUT") if events & IO::WRITABLE != 0
      bits |= __poll__("POLLPRI") if events & IO::PRIORITY != 0
      [io.__poll_fd__(events), bits]
    end
    deadline = timeout && Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    loop do
      ms = deadline.nil? ? -1 : ((deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)) * 1000).ceil.clamp(0, 2**31 - 1)
      # With another thread alive, the wait is the scheduler's, so that one
      # runs meanwhile (#45): a look now, then a wait for any of them.
      if __thread_others__
        revents = __fd_poll__(polls, 0)
        if Array === revents && revents.all?(0)
          return nil if ms == 0
          Thread.__before_wait__
          __thread_block__(deadline && ms / 1000.0, polls, 0, false)
          next
        end
      else
        revents = __fd_poll__(polls, ms)
      end
      if Integer === revents
        next if revents == Errno::EINTR::Errno
        __check__(revents, nil, "poll(2)")
      end
      return nil if revents.all?(0)
      done = __poll__("POLLERR") | __poll__("POLLHUP") | __poll__("POLLNVAL")
      return revents.each_with_index.map do |bits, i|
        events = pairs[i][1]
        ready = 0
        ready |= IO::READABLE if events & IO::READABLE != 0 && bits & (__poll__("POLLIN") | done) != 0
        ready |= IO::WRITABLE if events & IO::WRITABLE != 0 && bits & (__poll__("POLLOUT") | done) != 0
        ready |= IO::PRIORITY if events & IO::PRIORITY != 0 && bits & __poll__("POLLPRI") != 0
        ready
      end
    end
  end

  def self.select(reads, writes = nil, errors = nil, timeout = nil)
    sets = [reads, writes, errors].map do |set|
      next [] if set.nil?
      Array.try_convert(set) || raise(TypeError, "no implicit conversion of #{set.class} into Array")
    end
    ios = sets.map { |set| set.map { |item| IO.__to_io__(item) } }
    ios.each { |set| set.each(&:__check_open__) }
    pairs = []
    [IO::READABLE, IO::WRITABLE, IO::PRIORITY].each_with_index do |bit, i|
      ios[i].each { |io| pairs << [io, bit] }
    end
    unless timeout.nil?
      unless Numeric === timeout
        raise TypeError, "can't convert #{timeout.class} into time interval"
      end
      raise RangeError, "NaN out of Time range" if Float === timeout && timeout.nan?
      raise ArgumentError, "time interval must not be negative" if timeout < 0
    end
    if pairs.empty?
      timeout.nil? ? sleep : sleep(timeout)
      return nil
    end
    ready = __wait__(pairs, timeout)
    return nil if ready.nil?
    out = [[], [], []]
    k = 0
    [0, 1, 2].each do |i|
      sets[i].each_with_index do |item, j|
        out[i] << item if ready[k] != 0
        k += 1
      end
    end
    out
  end

  def self.__to_io__(item)
    return item if IO === item
    io = item.respond_to?(:to_io) ? item.to_io : nil
    raise TypeError, "no implicit conversion of #{item.class} into IO" unless IO === io
    io
  end

  def __buffered__? = !@rbuf.empty?

  # The descriptor to poll: a duplex child's write end for writing.
  def __poll_fd__(events) = events == IO::WRITABLE ? __wfd__ : @fileno

  def timeout = @timeout

  def timeout=(value)
    @timeout = value
  end

  def wait(*args)
    __check_open__
    if args.size == 2 || (args.size == 1 && Integer === args[0] && !(Float === args[0]))
      events = Integer.__index__(args[0])
      raise ArgumentError, "Events must be positive integer!" if events <= 0
      ready = IO.__wait__([[self, events]], args[1])
      return ready.nil? ? nil : ready[0]
    end
    # The old form: a timeout, and modes as Symbols.
    events = 0
    timeout = nil
    args.each do |arg|
      case arg
      when :r, :read, :readable then events |= IO::READABLE
      when :w, :write, :writable then events |= IO::WRITABLE
      when :rw, :read_write, :readable_writable then events |= IO::READABLE | IO::WRITABLE
      when Symbol then raise ArgumentError, "unsupported mode: #{arg}"
      else
        raise ArgumentError, "timeout given more than once" unless timeout.nil?
        timeout = arg
      end
    end
    events = IO::READABLE if events == 0
    ready = IO.__wait__([[self, events]], timeout)
    ready.nil? ? nil : self
  end

  def wait_readable(timeout = nil)
    __check_open__
    IO.__wait__([[self, IO::READABLE]], timeout).nil? ? nil : self
  end

  def wait_writable(timeout = nil)
    __check_open__
    IO.__wait__([[self, IO::WRITABLE]], timeout).nil? ? nil : self
  end

  def wait_priority(timeout = nil)
    __check_open__
    IO.__wait__([[self, IO::PRIORITY]], timeout).nil? ? nil : self
  end

  # `fcntl(2)`'s commands and flags, which are the platform's.
  def self.__fcntl__(name)
    (@__fcntl__ ||= __fd_fcntl_constants__.to_h)[name]
  end

  # An Integer bound for a C parameter, refused as CRuby's `NUM2INT`,
  # `NUM2UINT` and `NUM2LONG` refuse it.
  def self.__c_int__(value)
    value = Integer.__index__(value)
    raise RangeError, "integer #{value} too big to convert to 'int'" if value > 2147483647
    raise RangeError, "integer #{value} too small to convert to 'int'" if value < -2147483648
    value
  end

  def self.__c_uint__(value)
    value = Integer.__index__(value)
    raise RangeError, "integer #{value} too big to convert to 'unsigned int'" if value > 4294967295
    # A negative one wraps, as C's conversion does, down to `INT_MIN`.
    raise RangeError, "integer #{value} too small to convert to 'unsigned int'" if value < -2147483648
    value
  end

  def self.__c_long__(value)
    value = Integer.__index__(value)
    if value > 9223372036854775807 || value < -9223372036854775808
      raise RangeError, "bignum too big to convert into 'long'"
    end
    value
  end

  # A String argument: one, or what `to_str` makes of one.
  def self.__str__(value)
    return value if String === value
    return value.to_str if value.respond_to?(:to_str)
    raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into String"
  end

  # Raise the `SystemCallError` a primitive answered with an errno.
  def self.__check__(result, path = nil, where = nil)
    return result unless Integer === result
    # `strerror @ where - path`, which is how CRuby's `rb_syserr_fail_path`
    # names the C function that made the call.
    raise SystemCallError.new(path, result, where)
  end

  # --- making one ------------------------------------------------------------

def self.new(*args, **options, &block)
  __warning__("#{self}::new() does not take block; use #{self}::open() instead") unless block.nil?
  super(*args, **options, &nil)
end

def self.for_fd(*args, **options, &block)
  new(*args, **options, &block)
end

  def self.open(*args, **options)
    io = new(*args, **options)
    return io unless block_given?
    begin
      yield io
    ensure
      io.close unless io.closed?
    end
  end

  def self.sysopen(path, mode = nil, perm = nil)
    path = File.__path__(path)
    flags = mode.nil? ? File::RDONLY : IO.__parse_mode__(mode)[0]
    perm = perm.nil? ? 0666 : Integer.__index__(perm)
    IO.__check__(__fd_open__(path, flags, perm), path, "rb_sysopen")[0]
  end

  def self.pipe(*encodings, **options)
    fds = IO.__check__(__fd_pipe__)
    # Measured: made without `new`, so a class that redefines it is not asked.
    reader = allocate
    reader.__send__(:initialize, fds[0], "r", **options)
    writer = allocate
    writer.__send__(:initialize, fds[1], "w")
    writer.sync = true
    reader.set_encoding(*encodings, **options) unless encodings.empty?
    pair = [reader, writer]
    return pair unless block_given?
    begin
      yield pair
    ensure
      reader.close unless reader.closed?
      writer.close unless writer.closed?
    end
  end

  def self.try_convert(object)
    return object if IO === object
    return nil unless object.respond_to?(:to_io)
    io = object.to_io
    unless IO === io
      raise TypeError, "can't convert #{object.class} to IO (#{object.class}#to_io gives #{io.class})"
    end
    io
  end

  # `open(2)`'s flags for a mode and what the mode says about encodings:
  # `[flags, binmode, textmode, external, internal, bom]`. A String mode is
  # `r`, `w` or `a`, then any of `+`, `b`, `t` and (after `w`) `x`, then
  # `:external:internal`. CRuby's `rb_io_modestr_fmode` and
  # `rb_io_ext_int_to_encs`, measured.
  def self.__parse_mode__(mode)
    return [Integer.__index__(mode), false, false, nil, nil, false] unless String === mode || mode.respond_to?(:to_str)
    mode = mode.to_str unless String === mode
    letters, encodings = mode.split(":", 2)
    letters = letters.to_s
    invalid = -> { raise ArgumentError, "invalid access mode #{mode}" }
    flags = case letters[0]
            when "r" then File::RDONLY
            when "w" then File::WRONLY | File::CREAT | File::TRUNC
            when "a" then File::WRONLY | File::CREAT | File::APPEND
            else invalid.call
            end
    binmode = textmode = false
    letters[1..].each_char do |letter|
      case letter
      when "b" then binmode = true
      when "t" then textmode = true
      when "+" then flags = (flags & ~3) | File::RDWR
      when "x"
        invalid.call unless letters[0] == "w"
        flags |= File::EXCL
      else invalid.call
      end
    end
    invalid.call if binmode && textmode
    external = internal = nil
    bom = false
    unless encodings.nil?
      ext_name, int_name = encodings.split(":", 2)
      if ext_name.match?(/\Abom\|/i)
        bom = true
        ext_name = ext_name[4..]
      end
      external = __find_encoding__(ext_name)
      # "-" is no conversion, which is not the same as no internal
      # encoding: that one is the default.
      internal = int_name.nil? ? nil : (int_name == "-" ? :none : __find_encoding__(int_name))
    end
    [flags, binmode, textmode, external, internal, bom]
  end

  # An encoding named in a mode: an unknown one is warned about and ignored.
  def self.__find_encoding__(name)
    return name if Encoding === name
    Encoding.find(name)
  rescue ArgumentError
    __warning__("Unsupported encoding: #{name} ignored")
    nil
  end

  def initialize(fd, mode = nil, **options)
    fd = Integer.__index__(fd)
    current = IO.__check__(__fd_fcntl__(fd, IO.__fcntl__("F_GETFL"), 0))[0]
    if mode.nil? && !options.key?(:mode) && !options.key?(:flags)
      mode = current & 3
    end
    __setup__(fd, mode, options)
    # The descriptor has to allow what the mode asks for.
    wanted = @access
    have = current & 3
    if (wanted != 0 && have == 0) || (wanted != 1 && have == 1)
      raise Errno::EINVAL
    end
  end

  # What every way of making an IO shares: a descriptor and a mode, with the
  # options that refine it.
  def __setup__(fd, mode, options)
    # Measured: `mode: nil` beside a mode is no second mode.
    unless options[:mode].nil?
      raise ArgumentError, "mode specified twice" unless mode.nil?
      mode = options[:mode]
    end
    mode = "r" if mode.nil?
    flags, binmode, textmode, external, internal, bom = IO.__parse_mode__(mode)
    flags |= Integer.__index__(options[:flags]) if options[:flags]
    # `textmode:` and then `binmode:`, each against what is set so far,
    # whatever its value: CRuby's `rb_io_extract_modeenc`, measured.
    unless options[:textmode].nil?
      raise ArgumentError, "textmode specified twice" if textmode
      raise ArgumentError, "both textmode and binmode specified" if binmode
      textmode = true if options[:textmode]
    end
    unless options[:binmode].nil?
      raise ArgumentError, "binmode specified twice" if binmode
      raise ArgumentError, "both textmode and binmode specified" if textmode
      binmode = true if options[:binmode]
    end
    # The mode's encodings, then the options', then binmode's, then the
    # defaults: CRuby's `rb_io_extract_modeenc`.
    named = !external.nil? || !internal.nil? || bom
    if !options[:external_encoding].nil? || !options[:internal_encoding].nil?
      unless options[:encoding].nil?
        __warning__("Ignoring encoding parameter '#{options[:encoding]}': #{options[:external_encoding] ? "external" : "internal"}_encoding is used")
      end
      raise ArgumentError, "encoding specified twice" if named
      external = IO.__find_encoding__(options[:external_encoding]) unless options[:external_encoding].nil?
      wanted = options[:internal_encoding]
      internal = wanted.nil? ? nil : (wanted == "-" ? :none : IO.__find_encoding__(wanted))
      named = true
    elsif !options[:encoding].nil?
      raise ArgumentError, "encoding specified twice" if named
      encoding = options[:encoding]
      if Encoding === encoding
        external = encoding
      else
        _, _, _, external, internal, bom = IO.__parse_mode__("r:#{encoding.to_str}")
      end
      named = true
    end
    @fileno = fd
    # 0 reads, 1 writes, 2 does both: `O_ACCMODE`, the same on every target.
    @access = flags & 3
    @append = flags & File::APPEND != 0
    @binmode = binmode
    if named
      __ext_int_to_encs__(external, internal, bom)
    elsif binmode
      __ext_int_to_encs__(Encoding::BINARY, nil)
    else
      __ext_int_to_encs__(nil, nil)
    end
    @ecopts = __ecopts__(options)
    __validate_enc_binmode__
    @rbuf = "".b
    @lineno = 0
    @closed = false
    @sync = false
    @autoclose = options.key?(:autoclose) ? (options[:autoclose] ? true : false) : true
    path = options[:path]
    @path = (String === path ? path : File.__path__(path)).dup unless path.nil?
    __bom__ if bom && @access != 1
    self
  end

  # A copy has a descriptor of its own, `dup(2)` of the original's, at the
  # original's position. The two share an offset, so the original's read-ahead
  # is given back first, as CRuby's `rb_io_init_copy` flushes it.
  def initialize_copy(original)
    original.__check_open__
    original.__unread__
    @fileno = IO.__check__(__fd_dup__(original.fileno), @path)[0]
    @rbuf = "".b
    @closed = false
    @autoclose = true
    @ecopts = @ecopts.dup
    # A converter of its own, so an attribute's quote of its own.
    @xml_quote = nil
    self
  end

  # Seek the descriptor back over what is buffered and forget it. A pipe
  # cannot seek, and keeps its buffer.
  def __unread__
    return if @rbuf.empty?
    result = __fd_seek__(@fileno, -@rbuf.bytesize, IO::SEEK_CUR)
    @rbuf = "".b unless Integer === result
  end

  # The options that say how to convert, kept for `String#encode`.
  def __ecopts__(options)
    newline = options[:newline]
    unless newline.nil? || %i[universal crlf cr lf].include?(newline)
      raise ArgumentError, "unexpected value for newline option: #{newline}"
    end
    kept = {}
    %i[invalid undef replace xml newline universal_newline crlf_newline cr_newline lf_newline fallback].each do |key|
      kept[key] = options[key] if options.key?(key)
    end
    kept
  end

  # --- state -----------------------------------------------------------------

  def fileno
    __check_open__
    @fileno
  end
  alias to_i fileno

  def to_io = self

  def closed? = @closed

  def autoclose?
    __check_open__
    @autoclose
  end

  def autoclose=(value)
    __check_open__
    @autoclose = value ? true : false
    value
  end

  # A child's IO (`IO.popen`) waits for the child once it is closed, and
  # `$?` is how it went.
  def close
    return nil if @closed
    begin
      __close_quote__
    ensure
      @closed = true
      @rbuf = "".b
      __fd_close__(@wfileno) unless @wfileno.nil? || @wclosed
      IO.__check__(__fd_close__(@fileno), @path) if @autoclose && !@rclosed
      __reap__
    end
    nil
  end

  def __reap__
    return if @pid.nil?
    pid = @pid
    @pid = nil
    status = Process::Status.wait(pid)
    __global_store__(:$?, status) unless status.nil?
  end

  # The descriptor writes go to: a duplex child's own write end, or the one.
  def __wfd__ = @wfileno || @fileno

  def close_read
    return nil if @closed
    unless @wfileno.nil?
      __fd_close__(@fileno) unless @rclosed
      @rclosed = true
      @access = 1
      return @wclosed ? close : nil
    end
    raise IOError, "closing non-duplex IO for reading" unless @access == 0
    close
  end

  def close_write
    return nil if @closed
    unless @wfileno.nil?
      __close_quote__ unless @wclosed
      __fd_close__(@wfileno) unless @wclosed
      @wclosed = true
      @access = 0
      return @rclosed ? close : nil
    end
    raise IOError, "closing non-duplex IO for writing" unless @access == 1
    close
  end

  def close_on_exec?
    __check_open__
    flags = IO.__check__(__fd_fcntl__(@fileno, IO.__fcntl__("F_GETFD"), 0))[0]
    flags & IO.__fcntl__("FD_CLOEXEC") != 0
  end

  def close_on_exec=(value)
    __check_open__
    flags = IO.__check__(__fd_fcntl__(@fileno, IO.__fcntl__("F_GETFD"), 0))[0]
    cloexec = IO.__fcntl__("FD_CLOEXEC")
    flags = value ? flags | cloexec : flags & ~cloexec
    IO.__check__(__fd_fcntl__(@fileno, IO.__fcntl__("F_SETFD"), flags))
    value
  end

  def fcntl(command, argument = 0)
    __check_open__
    argument = case argument
               when true then 1
               when false, nil then 0
               when Integer then argument
               # ponytail: a String argument is a pointer to a struct, which
               # needs a primitive that copies it in and out.
               else raise NotImplementedError, "fcntl with a String argument"
               end
    IO.__check__(__fd_fcntl__(@fileno, Integer.__index__(command), argument), @path)[0]
  end

  # ponytail: every `ioctl` request is a pointer to something different.
  def ioctl(*)
    __check_open__
    raise NotImplementedError, "ioctl() function is unimplemented on this machine"
  end

  def pid
    __check_open__
    @pid
  end

  def __pid__(pid, writer = nil)
    @pid = pid
    @wfileno = writer
    self
  end

  def path
    @path.nil? ? nil : @path.dup
  end
  alias to_path path

  def sync
    __check_open__
    @sync
  end

  def sync=(value)
    __check_open__
    @sync = value ? true : false
    value
  end

  def fsync
    __check_open__
    IO.__check__(__fd_fsync__(@fileno, false), @path)
    0
  end

  def fdatasync
    __check_open__
    IO.__check__(__fd_fsync__(@fileno, true), @path)
    0
  end

  def flush
    __check_open__
    self
  end

  def tty?
    __check_open__
    __fs_isatty__(@fileno)
  end
  alias isatty tty?

  def stat
    __check_open__
    File::Stat.__from__(IO.__check__(__fs_stat__(@fileno, true), @path))
  end

  def binmode
    __check_open__
    @binmode = true
    @enc = Encoding::BINARY
    @enc2 = nil
    # CRuby drops the converter unfinished, an attribute's quote and all.
    @xml_quote = nil
    # And no conversion options at all: CRuby's `io_ascii8bit_binmode` zeroes
    # the flags and drops the rest, so `xml:` and the error handling go with
    # the newline decorators.
    @ecopts = {}
    self
  end

  def binmode?
    __check_open__
    @binmode
  end

  def inspect
    name = @path || { 0 => "<STDIN>", 1 => "<STDOUT>", 2 => "<STDERR>" }[@fileno] || "fd #{@fileno}"
    return "#<#{self.class}:#{@path.nil? ? "" : "#{@path} "}(closed)>" if @closed
    "#<#{self.class}:#{name}>"
  end

  def __check_open__
    raise IOError, "closed stream" if @closed
  end

  def __readable__
    __check_open__
    raise IOError, "not opened for reading" if @access == 1
  end

  def __writable__
    __check_open__
    raise IOError, "not opened for writing" if @access == 0
  end

  # --- encodings ---------------------------------------------------------------
  #
  # CRuby's two slots. `@enc` is the encoding reads answer in, nil for the
  # default external as it is when asked; `@enc2`, when there is one, is the
  # encoding the bytes are in, so reads convert from it to `@enc` and writes
  # back. Without `@enc2` nothing is converted on the way in.

  # `rb_io_ext_int_to_encs`: the slots for an external and an internal
  # encoding asked for. A nil external is the default, a nil internal the
  # default internal, and `:none` (an internal of "-") no conversion.
  def __ext_int_to_encs__(external, internal, bom = false)
    default_external = external.nil?
    external = Encoding.default_external if default_external
    if external == Encoding::BINARY
      internal = nil
    elsif internal.nil?
      internal = Encoding.default_internal
    end
    internal = nil if internal == :none
    if internal.nil? || (!bom && internal == external)
      @enc = default_external && internal != external ? nil : external
      @enc2 = nil
    else
      @enc = internal
      @enc2 = external
    end
  end

  # An IO that reads text in an encoding that is not ASCII-compatible has to
  # be in binmode: CRuby's `validate_enc_binmode`.
  def __validate_enc_binmode__
    if @access != 1 && @enc2.nil? && !@binmode && !(@enc || Encoding.default_external).ascii_compatible?
      raise ArgumentError, "ASCII incompatible encoding needs binmode"
    end
    if @binmode && __newline_decorator__?
      raise ArgumentError, "newline decorator with binary mode"
    end
  end

  def __newline_decorator__?
    @ecopts[:universal_newline] || @ecopts[:crlf_newline] || @ecopts[:cr_newline] || @ecopts[:lf_newline] || !@ecopts[:newline].nil?
  end

  # What reads answer in.
  def __read_encoding__
    @enc || Encoding.default_external
  end

  # What the bytes read are in.
  def __encoding__
    @enc2 || __read_encoding__
  end

  def external_encoding
    return @enc2 unless @enc2.nil?
    # One that writes has an encoding only when it was given one.
    return @enc if @access != 0
    __read_encoding__
  end

  def internal_encoding
    @enc2.nil? ? nil : __read_encoding__
  end

  # The encoding a name or an object stands for, as `set_encoding` takes one.
  def __encoding_arg__(value)
    return value if Encoding === value
    name = IO.__str__(value)
    raise ArgumentError, "invalid name encoding (non ASCII)" unless name.encoding.ascii_compatible?
    raise ArgumentError, "invalid encoding name (NUL byte)" if name.include?("\0")
    IO.__find_encoding__(name)
  end

  # CRuby's `io_encoding_set`, measured branch by branch.
  def set_encoding(*encodings, **options)
    __check_open__
    if encodings.size > 2 || encodings.empty?
      raise ArgumentError, "wrong number of arguments (given #{encodings.size}, expected 1..2)"
    end
    first, second = encodings
    ecopts = __kept_ecopts__(options)
    if !second.nil?
      external = __encoding_arg__(first)
      if !(Encoding === second) && second.to_str == "-"
        @enc = external
        @enc2 = nil
      else
        internal = __encoding_arg__(second)
        @enc = internal
        @enc2 = internal == external ? nil : external
      end
      # From BINARY there is nothing to convert.
      if @enc2 == Encoding::BINARY
        @enc = @enc2
        @enc2 = nil
      end
    elsif first.nil?
      __ext_int_to_encs__(nil, nil)
      ecopts = __kept_ecopts__({})
    elsif !(Encoding === first) && (String === first || first.respond_to?(:to_str))
      text = first.to_str
      raise ArgumentError, "invalid name encoding (non ASCII)" unless text.encoding.ascii_compatible?
      # A `bom|` here names the encoding and reads nothing: CRuby reads past a
      # mark only when it opens.
      _, _, _, external, internal, bom = IO.__parse_mode__("r:#{text}")
      __ext_int_to_encs__(external, internal, bom)
    else
      __ext_int_to_encs__(__encoding_arg__(first), nil)
      ecopts = __kept_ecopts__({})
    end
    @ecopts = ecopts
    # CRuby drops the converter unfinished: an attribute's quote open is
    # never closed, and the next write opens another.
    @xml_quote = nil
    __validate_enc_binmode__
    self
  end

  # The options `set_encoding` leaves, as CRuby's `io_encoding_set` starts
  # from the IO's own flags and `econv_opts` adds the given ones to them. The
  # flags (the error handling, `xml:`, the newline decorators) are kept; a
  # given one sets its own, and any newline option given replaces every
  # newline decorator kept. The replacement String and `fallback:` are not
  # flags, so only given ones count, and a `replace:` that stood for
  # `undef: :replace` leaves that behind with the default replacement.
  # `xml:` is flags too, so a second one that differs keeps both decorators,
  # `XML_BOTH`, for which a conversion then finds no converter (#458).
  def __kept_ecopts__(options)
    given = __ecopts__(options)
    newlines = %i[newline universal_newline crlf_newline cr_newline lf_newline]
    kept = @ecopts.reject { |key, _| %i[replace fallback].include?(key) }
    kept[:undef] = :replace if !@ecopts[:replace].nil? && @ecopts[:invalid] != :replace
    kept = kept.reject { |key, _| newlines.include?(key) } if newlines.any? { |key| !given[key].nil? }
    xml = kept[:xml]
    given.each { |key, value| kept[key] = value unless value.nil? }
    if %i[text attr].include?(given[:xml]) && (xml.equal?(XML_BOTH) || (%i[text attr].include?(xml) && xml != given[:xml]))
      kept[:xml] = XML_BOTH
    end
    kept
  end

  # Both `xml:` decorators at once, as an IO's flags can hold them: no
  # converter has the pair. Not a value `xml:` can be given.
  XML_BOTH = Object.new.freeze
  private_constant :XML_BOTH

  def set_encoding_by_bom
    __check_open__
    raise ArgumentError, "ASCII incompatible encoding needs binmode" unless @binmode
    raise ArgumentError, "encoding conversion is set" unless @enc2.nil?
    unless @enc.nil? || @enc == Encoding::BINARY
      raise ArgumentError, "encoding is set to #{@enc} already"
    end
    __bom__
  end

  BOMS = [
    ["\xEF\xBB\xBF".b, "UTF-8"],
    ["\x00\x00\xFE\xFF".b, "UTF-32BE"],
    ["\xFF\xFE\x00\x00".b, "UTF-32LE"],
    ["\xFE\xFF".b, "UTF-16BE"],
    ["\xFF\xFE".b, "UTF-16LE"],
  ].freeze
  private_constant :BOMS

  # Read past a byte order mark and take the encoding it names. Answers the
  # encoding, or nil when there is no mark.
  def __bom__
    nil while @rbuf.bytesize < 4 && __fill__(4 - @rbuf.bytesize)
    BOMS.each do |mark, name|
      next unless @rbuf.byteslice(0, mark.bytesize) == mark
      __take__(mark.bytesize)
      __ext_int_to_encs__(Encoding.find(name), nil, true)
      return Encoding.find(name)
    end
    nil
  end

  # Bytes read, in the IO's encoding and then in the one reads answer in.
  def __decode__(bytes)
    bytes.force_encoding(__encoding__)
    @enc2.nil? ? bytes : bytes.encode(__read_encoding__, **__read_ecopts__)
  end

  # The options a read transcodes with: CRuby's `make_readconv` leaves out
  # the decorators for writing. Universal newline is done on the way in.
  def __read_ecopts__
    options = @ecopts.reject { |key, _| %i[newline crlf_newline cr_newline lf_newline].include?(key) }
    if options[:xml].equal?(XML_BOTH)
      # Both `xml:` decorators, the quote's too, as CRuby's `make_readconv`
      # asks for all of them in one converter. Measured.
      newlines = options[:universal_newline] ? ["universal_newline"] : []
      raise String.__decorators_not_found__(@enc2, __read_encoding__, newlines + %w[xml_text xml_attr_content xml_attr_quote], nil)
    end
    options
  end

  # A String about to be written, in the encoding the IO writes: `@enc2`, or
  # `@enc` unless that is BINARY. CRuby's `do_writeconv`, with the newline
  # decorators run by `String#encode`. Measured.
  #
  # `xml: :attr`'s quote is the stream's, not each write's: CRuby's
  # `make_writeconv` keeps that stateful decorator in the IO's own converter,
  # made by the first write that is not empty, which opens the quote with the
  # first character it is given and closes it when the IO closes. So a write
  # is escaped without its quotes, and the first with anything in it opens
  # one. `@xml_quote` is nil before there is a converter, then whether the
  # quote is open.
  def __encode_out__(text)
    options = __write_ecopts__
    newline = String.__newline_decorators__(options).first
    target = @enc2 || (@enc == Encoding::BINARY ? nil : @enc)
    xml = options[:xml]
    if (xml == :attr || xml.equal?(XML_BOTH)) && @xml_quote.nil?
      @xml_quote = false
      IO.__open_quotes__[self] = true
    end
    if xml.equal?(XML_BOTH)
      # CRuby's `make_writeconv` gives the IO's own converter only the
      # stateful decorator, the quote, and that one it makes. A write first
      # converts with the other two, into the ASCII-compatible encoding the
      # IO's pairs with, and no converter has both: the quote is left to
      # close with nothing in it. Measured.
      source = text.encoding
      common = target.nil? ? source : Encoding::Converter.asciicompat_encoding(target) || target
      raise String.__decorators_not_found__(source, common, String.__newline_decorators__(options) + %w[xml_text xml_attr_content], nil)
    end
    if target.nil?
      # No conversion: the decorators work on the bytes, which a text mode
      # IO — any newline option makes one, `xml:` does not — wants
      # ASCII-compatible.
      if !text.encoding.ascii_compatible? && %i[newline universal_newline crlf_newline cr_newline lf_newline].any? { |key| @ecopts[key] }
        raise ArgumentError, "ASCII incompatible string written for text mode IO without encoding conversion: #{text.encoding}"
      end
      return text if newline.nil? && xml.nil?
      decorators = options.select { |key, _| %i[newline crlf_newline cr_newline lf_newline xml].include?(key) }
      return __open_quote__(text.b.encode(Encoding::BINARY, **decorators), xml).force_encoding(text.encoding)
    end
    ascii = Encoding::Converter.asciicompat_encoding(target)
    if ascii
      # Into an encoding that is not ASCII-compatible, CRuby converts twice,
      # even from that same encoding with no decorator, so an invalid String
      # raises: with the decorators to the ASCII-compatible encoding it pairs
      # with, then on to the IO's with only the error handling.
      handling = options.reject { |key, _| %i[newline crlf_newline cr_newline lf_newline xml].include?(key) }
      # The quote goes in before the second, so one that raises leaves it
      # open but unwritten, as CRuby's converter does. Measured.
      return __open_quote__(text.encode(ascii, **options), xml).encode(target, **handling)
    end
    # In its own encoding a String is still decorated: CRuby's `encode`
    # runs the decorators over its bytes, invalid ones and all.
    return text if newline.nil? && xml.nil? && text.encoding == target
    __open_quote__(text.encode(target, **options), xml)
  end

  # What `encode` with `xml: :attr` made, in an ASCII-compatible encoding,
  # as the stream's quote has it: without the quotes, and with the opening
  # one if this is the first character.
  def __open_quote__(quoted, xml)
    return quoted unless xml == :attr
    content = quoted.byteslice(1, quoted.bytesize - 2)
    return content if content.empty? || @xml_quote
    @xml_quote = true
    "\"".b.force_encoding(content.encoding) << content
  end

  # The closing quote CRuby's `finish_writeconv` writes when the IO closes,
  # or `""` if no character opened it.
  def __close_quote__
    open = @xml_quote
    return if open.nil?
    @xml_quote = nil
    quote = open ? "\"" : "\"\""
    target = @enc2 || (@enc == Encoding::BINARY ? nil : @enc)
    unless target.nil? || target.ascii_compatible?
      # The first character took UTF-16's or UTF-32's mark: the quote after
      # it is in the big-endian order they write.
      target = Encoding.find("#{target.name}BE") if open && (target == Encoding::UTF_16 || target == Encoding::UTF_32)
      quote = quote.encode(target)
    end
    __write_all__(quote)
  end

  # The IOs whose converter has `xml: :attr`'s quote, held weakly. CRuby
  # finishes an IO's converter when it finalizes the IO, so the program's end
  # closes the quote of each still open (#456).
  # ponytail: only at the end. An IO collected before it keeps its quote
  # open, as it keeps its descriptor, where CRuby closes both; upgrade by
  # finalizing an IO when it is collected (#460).
  def self.__open_quotes__
    @__open_quotes__ ||= ObjectSpace::WeakMap.new
  end

  # After the finalizers, as CRuby's IOs are finished after every other
  # object's. A failure is dropped, as a finalizer's is.
  def self.__finish_all__
    __open_quotes__.keys.each do |io|
      io.__finish__
    rescue Exception
    end
    nil
  end

  def __finish__
    __close_quote__ unless @closed || @wclosed
  end

  # The options a write transcodes with: CRuby's `make_writeconv` leaves out
  # the decorator for reading, universal newline, and `newline:` overrides
  # the other newline options.
  def __write_ecopts__
    return @ecopts.reject { |key, _| %i[newline universal_newline crlf_newline cr_newline lf_newline].include?(key) } if @ecopts[:newline] == :universal
    @ecopts.reject { |key, _| key == :universal_newline }
  end

  # --- the read buffer -----------------------------------------------------

  # Pull up to `want` more bytes into the buffer; false at the end of the file.
  def __fill__(want = 65536)
    chunk = IO.__check__(__read_wait__(want), @path)
    return false if chunk.empty?
    @rbuf << chunk
    true
  end

  # A read that waits for a nonblocking descriptor to have something, as
  # CRuby does for the sockets it makes nonblocking.
  def __read_wait__(want)
    loop do
      # A blocking descriptor would stop every thread: wait for it first.
      if __thread_others__
        IO.__wait__([[self, IO::READABLE]], @timeout, true) || raise(IO::TimeoutError, "Blocking operation timed out!")
        raise IOError, "stream closed in another thread" if @closed
      end
      chunk = __fd_read__(@fileno, want)
      return chunk unless chunk == IO.__eagain__
      IO.__wait__([[self, IO::READABLE]], @timeout) || raise(IO::TimeoutError, "Blocking operation timed out!")
    end
  end

  # Every byte of `text`, waiting whenever a nonblocking descriptor is full.
  def __write_all__(text)
    total = text.bytesize
    loop do
      IO.__wait__([[self, IO::WRITABLE]], @timeout, true) || raise(IO::TimeoutError, "Blocking operation timed out!") if __thread_others__
      result = __fd_write__(__wfd__, text)
      if result == IO.__eagain__
        IO.__wait__([[self, IO::WRITABLE]], @timeout) || raise(IO::TimeoutError, "Blocking operation timed out!")
        next
      end
      written = IO.__check__(result, @path)[0]
      return total if written >= text.bytesize
      text = text.byteslice(written, text.bytesize - written)
    end
  end

  def __take__(count)
    count = @rbuf.bytesize if count > @rbuf.bytesize
    taken = @rbuf.byteslice(0, count)
    @rbuf = @rbuf.byteslice(count, @rbuf.bytesize - count)
    taken
  end

  # Whether the first `cut` buffered bytes end part way through a UTF-8
  # character: a lead byte with fewer bytes after it than it announces.
  def __partial_character__(cut)
    back = 1
    while back <= 4 && back <= cut
      byte = @rbuf.getbyte(cut - back)
      if byte & 0xC0 != 0x80
        return false if byte < 0xC0
        needed = byte >= 0xF0 ? 4 : (byte >= 0xE0 ? 3 : 2)
        return back < needed
      end
      back += 1
    end
    false
  end

  # Universal newline on the way in, when it was asked for.
  def __newlines_in__(text)
    return text unless @ecopts[:universal_newline] || @ecopts[:newline] == :universal
    return String.__universal_newline__(text) if text.encoding.ascii_compatible?
    text.gsub(/\r\n?/, "\n")
  end

  # --- reading -------------------------------------------------------------

  def read(length = nil, buffer = nil)
    __readable__
    unless buffer.nil?
      buffer = buffer.to_str unless String === buffer
      raise FrozenError, "can't modify frozen String: #{buffer.inspect}" if buffer.frozen?
    end
    if length.nil?
      nil while __fill__
      out = __newlines_in__(__decode__(__take__(@rbuf.bytesize)))
    else
      length = Integer.__index__(length)
      raise ArgumentError, "negative length #{length} given" if length < 0
      while @rbuf.bytesize < length
        break unless __fill__(length - @rbuf.bytesize)
      end
      if @rbuf.empty? && length > 0
        buffer.clear unless buffer.nil?
        return nil
      end
      out = __take__(length)
    end
    return out if buffer.nil?
    # A sized read fills the caller's buffer and leaves its encoding alone.
    encoding = buffer.encoding
    buffer.replace(out)
    buffer.force_encoding(encoding) unless length.nil?
    buffer
  end

  # What is there, up to `length`: the buffer if it holds anything, or one
  # read from the descriptor. At the end, `EOFError`.
  def readpartial(length, buffer = nil)
    out = __partial__(length, buffer, "readpartial")
    raise EOFError, "end of file reached" if out.nil?
    out
  end

  def __partial__(length, buffer, name, blocking = true)
    __readable__
    length = Integer.__index__(length)
    raise ArgumentError, "negative length #{length} given" if length < 0
    unless buffer.nil?
      buffer = buffer.to_str unless String === buffer
      raise FrozenError, "can't modify frozen String: #{buffer.inspect}" if buffer.frozen?
    end
    if length == 0
      buffer&.clear
      return buffer || ""
    end
    if @rbuf.empty?
      chunk = blocking ? __read_wait__(length) : __fd_read__(@fileno, length)
      return :wait_readable if !blocking && chunk == IO.__eagain__
      IO.__check__(chunk, @path)
      if chunk.empty?
        buffer&.clear
        return nil
      end
      @rbuf << chunk
    end
    out = __take__(length < @rbuf.bytesize ? length : @rbuf.bytesize)
    return out if buffer.nil?
    # The caller's buffer keeps its encoding.
    encoding = buffer.encoding
    buffer.replace(out)
    buffer.force_encoding(encoding)
  end

  def self.__eagain__
    @__eagain__ ||= Errno::EAGAIN::Errno
  end

  def read_nonblock(length, buffer = nil, exception: true)
    __readable__
    IO.__exception_flag__(exception)
    __nonblock__
    out = __partial__(length, buffer, "read_nonblock", false)
    if out == :wait_readable
      return :wait_readable unless exception
      raise IO::EAGAINWaitReadable, "read would block"
    end
    if out.nil?
      return nil unless exception
      raise EOFError, "end of file reached"
    end
    out
  end

  def __nonblock__
    nonblock = File::NONBLOCK
    flags = IO.__check__(__fd_fcntl__(@fileno, IO.__fcntl__("F_GETFL"), 0))[0]
    IO.__check__(__fd_fcntl__(@fileno, IO.__fcntl__("F_SETFL"), flags | nonblock)) if flags & nonblock == 0
  end

  def sysread(length, buffer = nil)
    __readable__
    raise IOError, "sysread for buffered IO" unless @rbuf.empty?
    # Measured: a zero length hands the buffer back as it was.
    return buffer || "" if Integer.__index__(length) == 0
    readpartial(length, buffer)
  end

  def pread(length, offset, buffer = nil)
    __readable__
    length = Integer.__index__(length)
    offset = Integer.__index__(offset)
    raise ArgumentError, "negative string size (or size too big)" if length < 0
    # Measured: a zero length hands the buffer back as it was.
    return buffer || "" if length == 0
    chunk = IO.__check__(__fd_pread__(@fileno, length, offset), @path)
    raise EOFError, "end of file reached" if chunk.empty?
    return chunk if buffer.nil?
    # The caller's buffer keeps its encoding.
    encoding = buffer.encoding
    buffer.replace(chunk).force_encoding(encoding)
  end

  # Only `gets` and `readline` set `$_`: `readlines` and `each_line` leave it.
  def gets(*args, chomp: false, **)
    separator, limit = __line_args__(args)
    $_ = __gets__(separator, limit, chomp)
  end

  # `(separator = $/, limit = nil)`, where a lone Integer is the limit.
  # Measured: a negative limit is no limit.
  def __line_args__(args)
    if args.size > 2
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
    end
    separator = $/
    limit = nil
    if args.size == 1
      first = args[0]
      if first.nil? || String === first || first.respond_to?(:to_str)
        separator = first
      else
        limit = first
      end
    elsif args.size == 2
      separator, limit = args
    end
    separator = separator.to_str unless separator.nil? || String === separator
    limit = IO.__c_long__(limit) unless limit.nil?
    limit = nil if !limit.nil? && limit < 0
    [separator, limit]
  end

  def __gets__(separator, limit, chomp)
    __readable__
    return "" if limit == 0
    if separator.nil? && limit.nil?
      line = read
      return nil if line.empty?
      @lineno += 1
      $. = @lineno
      return line
    end
    # An empty separator is paragraph mode: a blank line ends the record.
    paragraph = !separator.nil? && separator.empty?
    # Blank lines before a record are not part of it.
    __take__(1) while paragraph && (@rbuf.empty? ? __fill__ : true) && @rbuf.getbyte(0) == 10
    wanted = nil
    unless separator.nil?
      wanted = paragraph ? "\n\n" : separator
      wanted = wanted.encode(__encoding__) unless wanted.encoding == __encoding__ || wanted.ascii_only? && __encoding__.ascii_compatible?
      wanted = wanted.b
    end
    from = 0
    cut = nil
    while cut.nil?
      at = wanted.nil? ? nil : @rbuf.index(wanted, from)
      if at
        cut = at + wanted.bytesize
      elsif !limit.nil? && limit > 0 && @rbuf.bytesize >= limit
        cut = limit
      else
        from = @rbuf.bytesize - (wanted.nil? ? 0 : wanted.bytesize) + 1
        from = 0 if from < 0
        cut = @rbuf.bytesize unless __fill__
      end
    end
    if !limit.nil? && limit > 0 && limit <= cut
      cut = limit
      # A limit that lands inside a character takes the rest of it: Ruby
      # does not hand back half a character. UTF-8 only, where the bytes
      # that continue one are recognisable.
      if __encoding__ == Encoding::UTF_8
        extra = 0
        while extra < 16 && __partial_character__(cut) && (cut < @rbuf.bytesize || __fill__)
          cut += 1
          extra += 1
        end
      end
    end
    return nil if cut == 0
    line = __newlines_in__(__decode__(__take__(cut)))
    if paragraph
      # The blank lines between records belong to neither.
      __take__(1) while (@rbuf.empty? ? __fill__ : true) && @rbuf.getbyte(0) == 10
    end
    # In paragraph mode `chomp` takes the blank line that ended the record and
    # nothing else: the last record keeps its own newline. Measured.
    line = line.chomp(paragraph ? "\n\n" : separator) if chomp && !separator.nil?
    @lineno += 1
    $. = @lineno
    line
  end

  def readline(*args, chomp: false, **)
    separator, limit = __line_args__(args)
    line = $_ = __gets__(separator, limit, chomp)
    raise EOFError, "end of file reached" if line.nil?
    line
  end

  def each_line(*args, chomp: false, **)
    # Without `chomp:` unless it was asked for: the enumerator hands keywords
    # back as a trailing Hash, which is one argument too many here.
    return enum_for(:each_line, *args) unless block_given? || chomp
    return enum_for(:each_line, *args, chomp: chomp) unless block_given?
    separator, limit = __line_args__(args)
    raise ArgumentError, "invalid limit: 0 for each_line" if limit == 0
    while (line = __gets__(separator, limit, chomp))
      yield line
    end
    self
  end
  alias each each_line

  def readlines(*args, chomp: false, **)
    separator, limit = __line_args__(args)
    raise ArgumentError, "invalid limit: 0 for readlines" if limit == 0
    lines = []
    while (line = __gets__(separator, limit, chomp))
      lines.push(line)
    end
    lines
  end

  def getbyte
    __readable__
    return nil if @rbuf.empty? && !__fill__
    __take__(1).getbyte(0)
  end

  def readbyte
    byte = getbyte
    raise EOFError, "end of file reached" if byte.nil?
    byte
  end

  def each_byte
    return enum_for(:each_byte) unless block_given?
    while (byte = getbyte)
      yield byte
    end
    self
  end

  # The bytes of the next character in the IO's encoding.
  def __char_length__
    encoding = __encoding__
    first = @rbuf.getbyte(0)
    case encoding
    when Encoding::UTF_8
      return 1 if first < 0xC2 || first > 0xF4
      first >= 0xF0 ? 4 : (first >= 0xE0 ? 3 : 2)
    when Encoding::UTF_16LE, Encoding::UTF_16BE
      high = encoding == Encoding::UTF_16LE ? @rbuf.getbyte(1).to_i : first
      high & 0xFC == 0xD8 ? 4 : 2
    when Encoding::UTF_32LE, Encoding::UTF_32BE then 4
    else
      return 1 if first < 0x80 || !encoding.ascii_compatible?
      # EUC-JP, Shift_JIS and the other multibyte encodings: the String's
      # own boundaries, over the longest any of their characters is.
      @rbuf.byteslice(0, 4).force_encoding(encoding).__char_len_at__(0)
    end
  end

  # Whether the buffer starts with a character of a multibyte encoding that
  # the bytes still to come would finish: `"\xA4"` in EUC-JP, where
  # `"\xA4A"` is a broken byte and then an `A`.
  def __char_cut_short__
    @rbuf.bytesize < 4 && @rbuf.getbyte(0) >= 0x80 &&
      @rbuf.byteslice(0, 4).force_encoding(__encoding__).__cut_short__
  end

  def getc
    __readable__
    return nil if @rbuf.empty? && !__fill__
    # The lead byte says how many more there are: reading further ahead
    # would wait on a pipe for bytes nobody has written.
    nil while @rbuf.bytesize < 2 && __encoding__.name.start_with?("UTF-16") && __fill__(1)
    nil while __char_cut_short__ && __fill__(1)
    length = __char_length__
    nil while @rbuf.bytesize < length && __fill__(length - @rbuf.bytesize)
    length = @rbuf.bytesize if length > @rbuf.bytesize
    char = __take__(length).force_encoding(__encoding__)
    return __broken_char__(char) unless char.valid_encoding?
    char = __newline_char_in__(char)
    @enc2.nil? ? char : char.encode(__read_encoding__, **__read_ecopts__)
  end

  # Universal newline on a character read alone: a CR is an LF, and takes
  # the LF after it with it. CRuby waits for the character that says which,
  # on a pipe as well. Measured.
  def __newline_char_in__(char)
    return char unless @ecopts[:universal_newline] || @ecopts[:newline] == :universal
    enc = char.encoding
    cr, lf = enc.ascii_compatible? ? ["\r".b, "\n".b] : ["\r".encode(enc).b, "\n".encode(enc).b]
    return char unless char.b == cr
    nil while @rbuf.bytesize < lf.bytesize && __fill__(lf.bytesize - @rbuf.bytesize)
    __take__(lf.bytesize) if @rbuf.byteslice(0, lf.bytesize) == lf
    lf.force_encoding(enc)
  end

  # Bytes that do not make a character are given back one at a time. Where
  # they are transcoded, the converter sees them with the bytes after, as
  # CRuby's does on the stream: `"\xA4A"` in EUC-JP is `"\xA4" followed by
  # "A"`, not an incomplete `"\xA4"`, and `"\xE3\x81A"` in UTF-8 is one broken
  # sequence of two bytes. The bytes its error names are read; the rest are
  # read again.
  def __broken_char__(char)
    @rbuf = char.b + @rbuf
    length = 1
    error = nil
    if @enc2
      size = char.bytesize
      loop do
        window = @rbuf.byteslice(0, size + 1).force_encoding(__encoding__)
        begin
          window.encode(__read_encoding__)
          break
        rescue Encoding::InvalidByteSequenceError => e
          error = e
          length = e.error_bytes.bytesize
          # Cut short at the end of what has been read: the converter waits
          # for the bytes that say what it is, and only then.
          break unless e.incomplete_input?
          size = window.bytesize
          break unless @rbuf.bytesize > size || __fill__(1)
        end
      end
    end
    char = __take__(length).force_encoding(__encoding__)
    return char if @enc2.nil?
    begin
      char.encode(__read_encoding__, **__read_ecopts__)
    rescue Encoding::InvalidByteSequenceError
      raise error || $!
    end
  end

  def readchar
    char = getc
    raise EOFError, "end of file reached" if char.nil?
    char
  end

  def each_char
    return enum_for(:each_char) unless block_given?
    while (char = getc)
      yield char
    end
    self
  end

  def each_codepoint
    return enum_for(:each_codepoint) unless block_given?
    while (char = getc)
      yield char.ord
    end
    self
  end

  def ungetc(char)
    __readable__
    bytes = if Integer === char
              (char.chr(__encoding__) rescue (char & 0xff).chr)
            else
              char = IO.__str__(char)
              char.encoding == __encoding__ || char.ascii_only? ? char : char.encode(__encoding__)
            end
    @rbuf = bytes.b + @rbuf
    nil
  end

  def ungetbyte(byte)
    __readable__
    return nil if byte.nil?
    bytes = Integer === byte ? (byte & 0xff).chr : (String === byte ? byte : byte.to_str)
    @rbuf = bytes.b + @rbuf
    nil
  end

  def eof?
    __readable__
    @rbuf.empty? && !__fill__
  end
  alias eof eof?

  def lineno
    __readable__
    @lineno
  end

  def lineno=(number)
    __readable__
    @lineno = IO.__c_int__(number)
  end

  # --- positions -------------------------------------------------------------

  def seek(offset, whence = IO::SEEK_SET)
    __check_open__
    whence = __whence__(whence)
    offset = IO.__c_long__(offset)
    # The descriptor is ahead of the reader by what is buffered.
    offset -= @rbuf.bytesize if whence == IO::SEEK_CUR
    IO.__check__(__fd_seek__(@fileno, offset, whence), @path)
    @rbuf = "".b
    0
  end

  def __whence__(whence)
    return whence unless Symbol === whence
    { SET: IO::SEEK_SET, CUR: IO::SEEK_CUR, END: IO::SEEK_END, DATA: IO::SEEK_DATA, HOLE: IO::SEEK_HOLE }.fetch(whence) do
      raise ArgumentError, "unknown whence: #{whence}"
    end
  end

  def sysseek(offset, whence = IO::SEEK_SET)
    __check_open__
    raise IOError, "sysseek for buffered IO" unless @rbuf.empty?
    IO.__check__(__fd_seek__(@fileno, IO.__c_long__(offset), __whence__(whence)), @path)[0]
  end

  def rewind
    seek(0)
    @lineno = 0
    0
  end

  def pos
    __check_open__
    IO.__check__(__fd_seek__(@fileno, 0, IO::SEEK_CUR), @path)[0] - @rbuf.bytesize
  end
  alias tell pos

  def pos=(position)
    seek(position)
    position
  end

  # --- writing ---------------------------------------------------------------

  def write(*objects)
    __check_open__
    # Measured: nothing to write is not checked against the mode.
    return 0 if objects.all? { |object| String === object && object.empty? }
    __writable__
    # Bytes read ahead are bytes the descriptor has moved past; a write
    # belongs where the reader is. A pipe cannot seek, and has no such place.
    __unread__ unless @rbuf.empty? || !@wfileno.nil?
    return 0 if objects.empty?
    if objects.size == 1
      text = objects[0]
      text = text.to_s unless String === text
      text = __encode_out__(text)
    else
      # One system call for all of them, as `writev` would be.
      text = "".b
      objects.each do |object|
        object = object.to_s unless String === object
        text << __encode_out__(object).b
      end
    end
    __write_all__(text)
  end

  def syswrite(object)
    __writable__
    text = String === object ? object : object.to_s
    IO.__check__(__fd_write__(__wfd__, text), @path)[0]
  end

  def write_nonblock(object, exception: true)
    __writable__
    IO.__exception_flag__(exception)
    __nonblock__
    text = String === object ? object : object.to_s
    result = __fd_write__(__wfd__, text)
    if result == IO.__eagain__
      return :wait_writable unless exception
      raise IO::EAGAINWaitWritable, "write would block"
    end
    IO.__check__(result, @path)[0]
  end

  def self.__exception_flag__(value)
    return if true.equal?(value) || false.equal?(value)
    raise ArgumentError, "expected true or false as exception: #{value.inspect}"
  end

  def pwrite(object, offset)
    __writable__
    text = String === object ? object : object.to_s
    IO.__check__(__fd_pwrite__(__wfd__, text, Integer.__index__(offset)), @path)[0]
  end

  def <<(object)
    write(object)
    self
  end

  def print(*objects)
    objects = [$_] if objects.empty?
    separator = $,
    out = []
    objects.each_with_index do |object, index|
      out << separator if index > 0 && !separator.nil?
      out << (object.nil? ? "" : object)
    end
    out << $\ unless $\.nil?
    # A write for each, as CRuby's `io_print` makes.
    out.each { |piece| write(piece) }
    nil
  end

  def printf(format = nil, *args)
    return nil if format.nil?
    write(Kernel.format(format, *args))
    nil
  end

  # A String's first character, or anything else as an Integer's low byte:
  # CRuby's `NUM2CHR`.
  def putc(char)
    write(String === char ? char[0].to_s : (Integer.__index__(char) & 0xff).chr)
    char
  end

  # An Array argument is flattened into its elements, measured: `puts [1, [2]]`
  # is two lines, `puts []` is none, and an Array that contains itself prints
  # `[...]` where it recurs rather than looping. One write for the lot.
  def puts(*lines)
    return write("\n") && nil if lines.empty?
    out = []
    __puts_lines__(lines, [], out)
    __writev__(out) unless out.empty?
    nil
  end

  # Several pieces in one `write`, as CRuby's `rb_io_writev`; a `write` that
  # takes one argument, the interface before Ruby 2.5, gets them one at a time.
  def __writev__(pieces)
    if pieces.size > 1 && method(:write).arity == 1
      pieces.each { |piece| write(piece) }
    else
      write(*pieces)
    end
  end

  def __puts_lines__(lines, seen, out)
    lines.each do |line|
      if String === line
        out << line
        out << "\n" unless line.__end_with_newline__
        next
      end
      array = Array === line ? line : (line.respond_to?(:to_ary, true) ? line.to_ary : nil)
      if Array === array
        if seen.any? { |outer| outer.equal?(array) }
          out << "[...]\n"
        else
          __puts_lines__(array, seen + [array], out)
        end
      else
        text = line.nil? ? "" : line.to_s
        text = Kernel.instance_method(:to_s).bind_call(line) unless String === text
        out << text
        out << "\n" unless text.__end_with_newline__
      end
    end
  end

  # --- another descriptor ----------------------------------------------------

  def reopen(other = nil, mode = nil, **options)
    if !(IO === other) && other.respond_to?(:to_io)
      converted = other.to_io
      unless IO === converted
        raise TypeError, "can't convert #{other.class} to IO (#{other.class}#to_io gives #{converted.class})"
      end
      other = converted
    end
    if IO === other
      other.__check_open__
      __check_open__
      position = other.__seekable_pos__
      unless other.fileno == @fileno
        IO.__check__(__fd_dup2__(other.fileno, @fileno), @path)
      end
      @access = other.__access__
      @path = other.path
      @rbuf = "".b
      IO.__check__(__fd_seek__(@fileno, position, IO::SEEK_SET)) unless position.nil?
      return self
    end
    path = File.__path__(other)
    mode = options[:mode] if mode.nil? && options.key?(:mode)
    mode = __mode_letters__ if mode.nil?
    flags = IO.__parse_mode__(mode)[0]
    fd = IO.__check__(__fd_open__(path, flags, 0666), path, "rb_sysopen")[0]
    if @closed
      @fileno = fd
      @closed = false
    else
      IO.__check__(__fd_dup2__(fd, @fileno), path)
      __fd_close__(fd)
    end
    __setup__(@fileno, mode, options.reject { |key, _| key == :mode })
    @path = path.dup
    self
  end

  def __access__ = @access

  # What `reopen` with a path opens when it was given no mode: the one this
  # IO has, without truncating what it opens.
  def __mode_letters__
    case @access
    when 0 then "r"
    when 1 then @append ? "a" : "w"
    else "r+"
    end
  end

  def __seekable_pos__
    pos
  rescue Errno::ESPIPE
    nil
  end

  # --- a child on the other end ----------------------------------------------

  # A command's standard output to read, its standard input to write, or
  # both, as one IO; `close` waits for it. The command is what `spawn` takes,
  # an Array holding its own options and environment included.
  # ponytail: `"-"`, which forks and runs the block on both sides, refuses.
  def self.popen(*args, **options, &block)
    env = Hash === args.first ? args.shift : {}
    # The options may come as a last positional Hash, as they did before
    # keywords.
    options = args.pop.merge(options) if args.size > 1 && Hash === args.last
    command = args.shift
    mode = args.shift
    raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if command.nil?
    mode = options.delete(:mode) if mode.nil? && options.key?(:mode)
    mode = "r" if mode.nil?
    # Converted once, here: it is read twice below.
    mode = mode.to_str unless String === mode || Integer === mode
    if command == "-"
      raise NotImplementedError, "IO.popen(\"-\"): a fork with the block on both sides is not supported yet"
    end
    spawn_args = Array === command ? command.dup : [IO.__str__(command)]
    command_options = Hash === spawn_args.last ? spawn_args.pop : {}
    if Hash === spawn_args.first
      env = env.merge(spawn_args.shift)
    end
    flags, = IO.__parse_mode__(mode)
    access = flags & 3
    io_keys = %i[external_encoding internal_encoding encoding textmode binmode autoclose invalid undef replace newline universal_newline crlf_newline cr_newline lf_newline xml]
    io_options = options.select { |key, _| io_keys.include?(key) }
    spawn_options = command_options.merge(options.reject { |key, _| io_keys.include?(key) })
    child_in = child_out = parent_r = parent_w = nil
    if access != 1
      parent_r, child_out = IO.__check__(__fd_pipe__)
    end
    if access != 0
      child_in, parent_w = IO.__check__(__fd_pipe__)
    end
    extra = []
    extra << [0, child_in] unless child_in.nil?
    extra << [1, child_out] unless child_out.nil?
    begin
      pid = Process::Spawn.start([env] + spawn_args + [spawn_options], extra, name: "popen")
    ensure
      __fd_close__(child_in) unless child_in.nil?
      __fd_close__(child_out) unless child_out.nil?
    end
    io = allocate
    if access == 2
      io.__setup__(parent_r, "r+", io_options)
      io.__pid__(pid, parent_w)
    elsif access == 1
      io.__setup__(parent_w, mode, io_options)
      io.__pid__(pid)
    else
      io.__setup__(parent_r, mode, io_options)
      io.__pid__(pid)
    end
    io.sync = true if access != 0
    return io unless block
    begin
      yield io
    ensure
      io.close unless io.closed?
    end
  rescue SystemCallError
    [parent_r, parent_w].each { |fd| __fd_close__(fd) unless fd.nil? } if pid.nil?
    raise
  end

  # --- reading and writing whole files -------------------------------------

  def self.__open_for__(name, mode, options)
    # `open_args:` is everything `open` is given after the name.
    if options.key?(:open_args)
      args = Array(options[:open_args])
      keywords = Hash === args.last ? args.pop : {}
      return File.new(name, *args, **keywords)
    end
    options = options.dup
    mode = options.delete(:mode) if options.key?(:mode)
    perm = options.delete(:perm)
    File.new(name, mode, perm, **options)
  end

  def self.read(name, length = nil, offset = nil, **options)
    # The offset is checked first, measured.
    unless offset.nil?
      offset = Integer.__index__(offset)
      raise ArgumentError, "negative offset #{offset} given" if offset < 0
    end
    unless length.nil?
      length = Integer.__index__(length)
      raise ArgumentError, "negative length #{length} given" if length < 0
    end
    file = __open_for__(name, "r", options)
    begin
      file.seek(offset) unless offset.nil?
      file.read(length)
    ensure
      file.close
    end
  end

  def self.binread(name, length = nil, offset = nil)
    read(name, length, offset, mode: "rb")
  end

  def self.write(name, data, offset = nil, **options)
    unless options.key?(:mode)
      options = options.merge(mode: offset.nil? ? "w" : File::WRONLY | File::CREAT)
    end
    file = __open_for__(name, nil, options)
    begin
      file.seek(Integer.__index__(offset)) unless offset.nil?
      file.write(data)
    ensure
      file.close
    end
  end

  def self.binwrite(name, data, offset = nil, **options)
    write(name, data, offset, binmode: true, **options)
  end

  def self.readlines(name, *args, chomp: false, **options)
    file = __open_for__(name, "r", options)
    begin
      file.readlines(*args, chomp: chomp)
    ensure
      file.close
    end
  end

  def self.foreach(name, *args, chomp: false, **options, &block)
    return enum_for(:foreach, name, *args, chomp: chomp, **options) unless block
    file = __open_for__(name, "r", options)
    begin
      file.each_line(*args, chomp: chomp, &block)
    ensure
      file.close
    end
    # Measured: what it leaves in `$_` is nil.
    $_ = nil
  end

  # From one IO or file to another, a block at a time. A source offset reads
  # with `pread` and leaves the source where it was, as CRuby does.
  def self.copy_stream(source, destination, length = nil, offset = nil)
    close_source = close_destination = false
    unless IO === source || source.respond_to?(:read) || source.respond_to?(:readpartial)
      source = File.open(File.__path__(source), "rb")
      close_source = true
    end
    unless IO === destination || destination.respond_to?(:write)
      destination = File.open(File.__path__(destination), "wb")
      close_destination = true
    end
    length = Integer.__index__(length) unless length.nil?
    copied = 0
    begin
      position = offset.nil? ? nil : Integer.__index__(offset)
      while length.nil? || copied < length
        want = length.nil? ? 65536 : [65536, length - copied].min
        chunk = if !position.nil? && IO === source
                  begin
                    source.pread(want, position + copied)
                  rescue EOFError
                    nil
                  end
                elsif IO === source || source.respond_to?(:readpartial)
                  begin
                    source.readpartial(want)
                  rescue EOFError
                    nil
                  end
                else
                  source.read(want)
                end
        break if chunk.nil? || chunk.empty?
        destination.write(chunk)
        copied += chunk.bytesize
      end
    ensure
      source.close if close_source
      destination.close if close_destination
    end
    copied
  end
end

# The standard streams. Made without `new`, which checks the descriptor: a
# process may start with one of them closed.
STDIN = IO.allocate.__setup__(0, "r", { path: "<STDIN>" })
STDOUT = IO.allocate.__setup__(1, "w", { path: "<STDOUT>" })
STDERR = IO.allocate.__setup__(2, "w", { path: "<STDERR>" })
# Measured: only STDERR starts unbuffered.
STDERR.sync = true
$stdin = STDIN
$stdout = STDOUT
$stderr = STDERR
alias $> $stdout
# `-W1`, which is the default: `warn` prints, and verbose-only warnings do not.
$VERBOSE = false
alias $-v $VERBOSE
alias $-w $VERBOSE
# The input record separator: what `gets` and `each_line` split on.
$/ = "\n"
alias $-0 $/

module Kernel
  def puts(*lines)
    $stdout.puts(*lines)
  end

  def print(*parts)
    $stdout.print(*parts)
  end

  def p(*values)
    values.each { |value| $stdout.write(value.inspect, "\n") }
    return nil if values.empty?
    values.size == 1 ? values[0] : values
  end

  def warn(*messages, uplevel: nil, category: nil)
    unless uplevel.nil?
      level = Integer.__index__(uplevel)
      raise ArgumentError, "negative level (#{level})" if level < 0
    end
    unless category.nil? || Symbol === category
      raise TypeError, "no implicit conversion of #{category.class} into Symbol" unless category.respond_to?(:to_sym)
      category = category.to_sym
    end
    return nil if $VERBOSE.nil? || messages.empty?
    return nil if !category.nil? && !Warning[category]
    text = +""
    unless uplevel.nil?
      # `uplevel` frames out is one further down. Past the end of the stack
      # there is nothing to name, and Ruby says only `warning: `.
      here = __backtrace_here__[level]
      text << (here.nil? ? "warning: " : "#{here[0]}:#{here[1]}: warning: ")
    end
    __warn_lines__(messages, text, [])
    # `Kernel#warn` bound to `Warning` itself writes rather than asking
    # `Warning.warn`, which is what ends the recursion when a program's
    # `Warning#warn` calls `super`.
    if Warning.equal?(self)
      $stderr.write(text)
    else
      __warning_send__(text, category)
    end
    nil
  end

  # `IO#puts`'s line rules, into a String: an Array is its elements, and a
  # line gets a newline unless it ends in one.
  def __warn_lines__(messages, text, seen)
    messages.each do |message|
      if message.is_a?(Array)
        if seen.any? { |outer| outer.equal?(message) }
          text << "[...]\n"
        else
          __warn_lines__(message, text, seen + [message])
        end
      else
        line = message.nil? ? "" : message.to_s
        text << line
        text << "\n" unless line.end_with?("\n")
      end
    end
  end

  def printf(*args)
    return nil if args.empty?
    target = args[0].is_a?(String) ? $stdout : args.shift
    target.write(Kernel.format(*args))
    nil
  end

  def putc(char)
    $stdout.putc(char)
  end

  # ponytail: a path only. `open("|command")` is a process (#43).
  def open(path, *rest, **options, &block)
    # CRuby's `rb_f_open`: what `to_open` answers is the IO, given to the
    # block and closed after it if it can be.
    if !(String === path) && path.respond_to?(:to_open)
      io = path.to_open(*rest, **options)
      return io unless block
      begin
        return yield(io)
      ensure
        io.close if io.respond_to?(:close)
      end
    end
    File.open(path, *rest, **options, &block)
  end

  # The next line of the files named in `ARGV`, each taken off as it is
  # opened, or of standard input when there are none: what `ruby -n` loops
  # over.
  # ponytail: the reading half of `ARGF` only, enough for `gets`; `ARGF`
  # itself, with `lineno`, `filename`, `skip` and in-place editing, is #41's.
  def gets(*args, **options)
    loop do
      source = Kernel.__argf_source__
      return $_ = nil if source.nil?
      line = source.gets(*args, **options)
      unless line.nil?
        $. = Kernel.__argf_lineno__ += 1
        return $_ = line
      end
      return $_ = nil if source.equal?($stdin)
      source.close
      Kernel.__argf_current__ = nil
    end
  end

  def readline(*args, **options)
    line = gets(*args, **options)
    raise EOFError, "end of file reached" if line.nil?
    line
  end

  def readlines(*args, **options)
    lines = []
    while (line = gets(*args, **options))
      lines << line
    end
    lines
  end

  class << self
    attr_accessor :__argf_current__
    attr_writer :__argf_lineno__

    def __argf_lineno__ = @__argf_lineno__ || 0

    def __argf_source__
      return @__argf_current__ unless @__argf_current__.nil?
      if ARGV.empty?
        return nil if @__argf_read_stdin__
        @__argf_read_stdin__ = true
        return @__argf_current__ = $stdin
      end
      name = ARGV.shift
      @__argf_read_stdin__ = true
      @__argf_current__ = name == "-" ? $stdin : File.open(name)
    end
  end

  module_function :puts, :print, :p, :warn, :printf, :putc, :open, :gets, :readline, :readlines
end
