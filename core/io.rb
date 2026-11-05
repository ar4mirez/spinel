# `IO` is a descriptor (#41): opened by something else and handed over as an
# Integer, or by `File`, `IO.sysopen` and `IO.pipe`. Reads are buffered,
# writes go straight out, and every system call is one primitive that
# answers an errno on failure. The rest, which is most of it, is Ruby.
#
# The read buffer is `@rbuf`, bytes the descriptor has moved past and the
# reader has not: `pos` gives it back and counts what stays, a relative seek
# adjusts for it, and a write after a read seeks back over it first.
#
# ponytail: no write buffer, so `flush` has nothing to do and `sync` only says
# what a `write` of several does with what it has gathered; and conversion between an external and an internal encoding
# is done a whole read or a whole line at a time, by `String#encode`, unless
# one of them is stateful or `reopen` left one, where the IO keeps a converter
# (`__stream_in__?`).
# That stream decoder for every read is the upgrade, when a spec needs a line
# split across a conversion.

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
    ios[1].each(&:__check_write_open__)
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
    __readable__
    IO.__wait__([[self, IO::READABLE]], timeout).nil? ? nil : self
  end

  # CRuby's duplex child is two IOs, and this looks at the side that reads,
  # however much a write goes through to the other, until `reopen` gives that
  # side a mode of its own (`@rmoded`).
  def wait_writable(timeout = nil)
    __writable__
    raise IOError, "not opened for writing" if __tied__? && !@rmoded
    IO.__wait__([[self, IO::WRITABLE]], timeout).nil? ? nil : self
  end

  def wait_priority(timeout = nil)
    __readable__
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

  # Measured: made without `new`, and initialized by IO's own `initialize`,
  # as CRuby's `rb_io_s_for_fd` calls `rb_io_initialize`, so neither a
  # redefined `new` nor a subclass's `initialize` is asked.
  def self.for_fd(*args, **options)
    io = allocate
    IO.instance_method(:initialize).bind_call(io, *args, **options)
    io
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
    flags = mode.nil? ? File::RDONLY : IO.__mode_flags__(mode)[0]
    perm = perm.nil? ? 0666 : Integer.__index__(perm)
    IO.__check__(__fd_open__(path, flags, perm), path, "rb_sysopen")[0]
  end

  def self.pipe(*encodings, **options)
    fds = IO.__check__(__fd_pipe__)
    # Measured: made without `new`, so a class that redefines it is not asked.
    reader = allocate
    # These are read with the encodings, by `set_encoding`, or not at all.
    ignored = %i[invalid undef replace xml newline universal_newline crlf_newline cr_newline lf_newline external_encoding internal_encoding]
    reader.__send__(:initialize, fds[0], "r", **options.reject { |key, _| ignored.include?(key) })
    writer = allocate
    # By number: a pipe's mode says nothing of creating or truncating, which
    # `reopen` with a path and no mode goes by.
    writer.__send__(:initialize, fds[1], File::WRONLY)
    writer.sync = true
    begin
      reader.set_encoding(*encodings, **options) unless encodings.empty?
    rescue Exception
      reader.close
      writer.close
      raise
    end
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
    io = Kernel.__check_funcall__(object, :to_io) { nil }
    unless nil.equal?(io) || IO === io
      klass = Kernel.__class_of__(object)
      raise TypeError, "can't convert #{klass} to IO (#{klass}#to_io gives #{Kernel.__class_of__(io)})"
    end
    io
  end

  # `open(2)`'s flags for a mode and what the mode says about encodings:
  # `[flags, binmode, textmode, external, internal, bom, named]`, `named` being
  # whether it has an encodings part at all, found or not. A String mode is
  # `r`, `w` or `a`, then any of `+`, `b`, `t` and (after `w`) `x`, then
  # `:external:internal`. CRuby's `rb_io_modestr_fmode` and `parse_mode_enc`,
  # measured.
  def self.__parse_mode__(mode)
    flags, binmode, textmode, encodings = __mode_flags__(mode)
    external = internal = nil
    bom = false
    unless encodings.nil?
      # The last colon splits them, so a name before it may hold another.
      colon = encodings.rindex(":")
      ext_name = colon ? encodings[0...colon] : encodings
      int_name = colon && encodings[(colon + 1)..]
      # A prefix naming nothing is no prefix, and only a name that starts
      # `UTF-` has a mark looked for, however else it might be spelled.
      if ext_name.match?(/\Abom\|./im)
        ext_name = ext_name[4..]
        bom = ext_name.match?(/\Autf-/i)
        __warning__("BOM with non-UTF encoding #{encodings[4..]} is nonsense") unless bom
      end
      # An empty name is warned about as all of what it was split from.
      external = __find_encoding__(ext_name, ext_name.empty? ? encodings : ext_name)
      # "-" is no conversion, which is not the same as no internal
      # encoding: that one is the default.
      internal = int_name.nil? ? nil : (int_name == "-" ? :none : __find_encoding__(int_name))
    end
    [flags, binmode, textmode, external, internal, bom, !encodings.nil?]
  end

  # The part of a mode that needs no encoding found, so what only opens warns
  # about none: `[flags, binmode, textmode, encodings]`.
  def self.__mode_flags__(mode)
    return [Integer.__index__(mode), false, false, nil] unless String === mode || mode.respond_to?(:to_str)
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
    [flags, binmode, textmode, encodings]
  end

  # An encoding named in a mode: an unknown one is warned about and ignored.
  def self.__find_encoding__(name, shown = name)
    return name if Encoding === name
    Encoding.find(name)
  rescue ArgumentError
    __warning__("Unsupported encoding #{shown} ignored")
    nil
  end

  # An encoding named by an option: an unknown one is refused, which only a
  # mode's is not. CRuby's `rb_to_encoding`.
  def self.__to_encoding__(value)
    return value if Encoding === value
    name = String.__coerce__(value)
    raise ArgumentError, "invalid encoding name (non ASCII)" unless name.encoding.ascii_compatible?
    raise ArgumentError, "invalid encoding name (NUL byte)" if name.include?("\0")
    Encoding.find(name)
  end

  # `internal_encoding:`, which CRuby reads as a C string before it looks for
  # the encoding, so "-" is no conversion however the String is encoded: a
  # wide one ends at its first NUL byte.
  def self.__internal_option__(value)
    return value if Encoding === value
    name = String.__coerce__(value)
    if name.encoding.ascii_compatible? || name.encoding.dummy?
      raise ArgumentError, "string contains null byte" if name.b.include?("\0")
    elsif name.each_char.any? { |char| char.b.delete("\0").empty? }
      raise ArgumentError, "string contains null char"
    end
    name.b.partition("\0")[0] == "-" ? :none : __to_encoding__(value)
  end

  def initialize(fd, mode = nil, **options)
    fd = Integer.__index__(fd)
    current = IO.__check__(__fd_fcntl__(fd, IO.__fcntl__("F_GETFL"), 0))[0]
    if mode.nil? && !options.key?(:mode) && !options.key?(:flags)
      mode = current & (3 | File::APPEND)
    end
    __setup__(fd, mode, options)
    # The descriptor has to allow what the mode asks for.
    wanted = @access
    have = current & 3
    if (wanted != 0 && have == 0) || (wanted != 1 && have == 1)
      # Not this IO's to close: CRuby checks before the IO takes it.
      @finalizer&.fd = nil
      raise Errno::EINVAL
    end
  end

  # What every way of making an IO shares: a descriptor and a mode, with the
  # options that refine it.
  def __setup__(fd, mode, options)
    __open_fd__(fd, options, __modeenc__(mode, options))
  end

  # CRuby's `rb_io_extract_modeenc`, which reads the mode and the options
  # before anything is opened or started, so one it refuses creates no file,
  # truncates none and runs no child. Answers whether the mode asks for a BOM.
  def __modeenc__(mode, options)
    # Measured: `mode: nil` beside a mode is no second mode.
    unless options[:mode].nil?
      raise ArgumentError, "mode specified twice" unless mode.nil?
      mode = options[:mode]
    end
    mode = "r" if mode.nil?
    flags, binmode, textmode, external, internal, bom, named = IO.__parse_mode__(mode)
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
    if !options[:external_encoding].nil? || !options[:internal_encoding].nil?
      unless options[:encoding].nil?
        __warning__("Ignoring encoding parameter '#{options[:encoding]}': #{options[:external_encoding] ? "external" : "internal"}_encoding is used")
      end
      given = IO.__to_encoding__(options[:external_encoding]) unless options[:external_encoding].nil?
      wanted = options[:internal_encoding]
      wanted = IO.__internal_option__(wanted) unless wanted.nil?
      raise ArgumentError, "encoding specified twice" if named
      external = given
      internal = wanted
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
    ecopts = __ecopts__(options)
    # 0 reads, 1 writes, 2 does both: `O_ACCMODE`, the same on every target.
    access = flags & 3
    if named
      enc, enc2 = __ext_int_to_encs__(external, internal, bom)
    elsif binmode
      enc, enc2 = __ext_int_to_encs__(Encoding::BINARY, nil)
    else
      enc, enc2 = __ext_int_to_encs__(nil, nil)
    end
    # A text mode IO reads with universal newline, on every platform, unless
    # a newline option says otherwise: CRuby's `rb_io_extract_modeenc` gives
    # its flags the decorator before the options are looked at (#465).
    if textmode && access != 1 && %i[newline universal_newline crlf_newline cr_newline lf_newline].all? { |key| ecopts[key].nil? }
      ecopts[:universal_newline] = true
    end
    # Checked before any of it is assigned: `reopen` keeps what it refuses.
    __validate_enc_binmode__(access, binmode, enc, enc2, ecopts)
    yield flags, binmode, ecopts if block_given?
    @access = access
    @binmode = binmode
    @enc = enc
    @enc2 = enc2
    @ecopts = ecopts
    # What makes a write want an ASCII-compatible String, on a duplex child's
    # write side too, which has no decorator of its own: a newline decorator
    # here, so `"r+t"` but not `"wt"`, which has none. Measured.
    @textmode = __newline_decorator__? ? true : false
    @oflags = flags
    bom
  end

  # The descriptor of an IO `__modeenc__` has read the mode of.
  def __open_fd__(fd, options, bom)
    @fileno = fd
    @rbuf = "".b
    @cbuf = "".b
    @lineno = 0
    @closed = false
    @sync = false
    @autoclose = options.key?(:autoclose) ? (options[:autoclose] ? true : false) : true
    path = options[:path]
    @path = (String === path ? path : File.__path__(path)).dup unless path.nil?
    # CRuby's `io_check_tty`: only here, so a `reopen` onto a terminal is not.
    @duplex = true if __fs_isatty__(fd)
    __bom__ if bom
    __arm__
    self
  end

  # The descriptor this IO owns from now on, for its finalizer (#460). An IO
  # `reopen` sets up again keeps the one it has, and the attribute's quote
  # still open, which `close` would write to the new file (#488).
  def __arm__
    if @finalizer.nil?
      @finalizer = Finalizer.new
      __finalizer_add__(self, @finalizer)
    end
    @finalizer.fd = @fileno
    @finalizer.wfd = nil
    @finalizer.autoclose = @autoclose
    @finalizer.quote = __closing_quote__(@xml_quote)
    @finalizer.writeconv = @writeconv
  end

  # A copy has a descriptor of its own, `dup(2)` of the original's, at the
  # original's position. The two share an offset, so what the original's writes
  # in progress have gathered is written and its read-ahead given back first, as
  # CRuby's `rb_io_init_copy` flushes both, and what that finds out about the
  # descriptor the copy is told too.
  def initialize_copy(original)
    original.__check_open__
    original.__flush_gathered__
    original.__unread__(true)
    @duplex = original.__duplex__
    @fileno = IO.__check__(__fd_dup__(original.fileno), @path)[0]
    @rbuf = "".b
    @cbuf = "".b
    __drop_readconv__
    @closed = false
    @autoclose = true
    @ecopts = @ecopts.dup
    # A converter of its own, so an attribute's quote of its own.
    @writeconv = nil
    @wpre = nil
    @xml_quote = nil
    # The writes in progress are the original's.
    @wgathered = nil
    @wsynced = false
    # And a finalizer of its own, for the descriptor of its own.
    @finalizer = nil
    # A copy is no standard stream, whatever it copies.
    @stdio = nil
    __arm__
    self
  end

  # CRuby's `io_unread`: seek the descriptor back over what is buffered and
  # forget it, and both converters with it. What cannot seek keeps its buffer
  # unless it is to be discarded all the same. `@duplex` is `FMODE_DUPLEX`, of
  # a socket, a tty or a child, or of what this finds cannot seek, and such an
  # IO keeps its buffer from then on, whatever its descriptor comes to be.
  def __unread__(discard = false)
    return if @rbuf.empty? || @duplex
    result = __fd_seek__(@fileno, -@rbuf.bytesize, IO::SEEK_CUR)
    if Integer === result
      @duplex = true if result == Errno::ESPIPE::Errno
      return unless discard
    end
    @rbuf = "".b
    __clear_codeconv__
  end

  def __duplex__ = @duplex

  # The options that say how to convert, kept for `String#encode`.
  def __ecopts__(options)
    String.__econv_opts__(options)
    kept = {}
    %i[invalid undef replace xml newline universal_newline crlf_newline cr_newline lf_newline fallback].each do |key|
      kept[key] = options[key] if options.key?(key)
    end
    # CRuby's `rb_econv_prepare_options` refuses a broken one here, and keeps a
    # frozen copy, rather than leaving that to a conversion that uses it.
    unless kept[:replace].nil?
      replacement = String.__replacement_option__(kept[:replace])
      kept[:replace] = replacement.frozen? ? replacement : replacement.dup.freeze
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
    @finalizer&.autoclose = @autoclose
    value
  end

  # A child's IO (`IO.popen`) waits for the child once it is closed, and
  # `$?` is how it went: nil when another IO for the child reaped it first.
  def close
    return nil if @closed
    begin
      __finish_writeconv__
    ensure
      @closed = true
      # The access goes and the rest of the mode stays, for `reopen`.
      @access = 3
      @stdio = nil
      @finalizer&.disarm
      @rbuf = "".b
      @cbuf = "".b
      # CRuby's `close` leaves `readconv` freed, and a `reopen` crashes on it.
      __drop_readconv__
      unless @wfileno.nil? || @wclosed
        __fd_close__(@wfileno)
        # The write end is gone for good, so a `reopen` does not take it back.
        # CRuby's `close` leaves the tied write IO closed, not untied, and a
        # write after `reopen` finds it so; `close_write` and `close_read`
        # untie it.
        @wclosed = true
        @wgone = true unless @rclosed
        @finalizer&.wfd = nil
      end
      IO.__check__(__fd_close__(@fileno), @path) if @autoclose && !@rclosed && @fileno > 2
      __reap__
    end
    nil
  end

  def __reap__
    return if @pid.nil?
    pid = @pid
    @pid = nil
    __global_store__(:$?, Process::Status.wait(pid))
  end

  # The descriptor writes go to: a duplex child's own write end, or the one.
  def __wfd__ = @wclosed ? @fileno : (@wfileno || @fileno)

  # Whether this is a duplex child still read from. CRuby makes that two IOs,
  # and the one `tied_io_for_writing` holds has no encoding and no options:
  # what `IO.popen` was given, and `set_encoding`, are the read side's, so a
  # write goes out as it is. `@textmode` is both sides', and `@wbinary` says
  # the write side has been made binary, for when `close_read` leaves only it.
  def __tied__? = !@wfileno.nil? && !@rclosed

  def __rmoded__ = @rmoded

  def close_read
    return nil if @closed
    unless @wfileno.nil?
      unless @rclosed
        __fd_close__(@fileno)
        # What is left is CRuby's write side, whose encodings are its own.
        @enc = @wbinary ? Encoding::BINARY : nil
        @enc2 = nil
        @binmode = true if @wbinary
        @ecopts = {}
      end
      @rclosed = true
      @finalizer&.fd = nil
      @access = 1
      return @wclosed ? close : nil
    end
    raise IOError, "closing non-duplex IO for reading" if __writes__?
    close
  end

  def close_write
    return nil if @closed || @wgone
    unless @wfileno.nil?
      # A second `close_write` finds the write side untied and closes the IO
      # itself, as CRuby's does once `GetWriteIO` gives back the reader.
      return close if @wclosed
      __finish_writeconv__
      __fd_close__(@wfileno)
      @wclosed = true
      @finalizer&.wfd = nil
      @access = 0
      return @rclosed ? close : nil
    end
    raise IOError, "closing non-duplex IO for writing" if __reads__?
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
    @finalizer&.wfd = writer
    @duplex = true
    self
  end

  def path
    @path.nil? ? nil : @path.dup
  end
  alias to_path path

  def sync
    __check_write_open__
    @sync
  end

  def sync=(value)
    __check_write_open__
    @sync = value ? true : false
    value
  end

  # What writes in progress have gathered goes first, as CRuby's `rb_io_fsync`
  # flushes its write buffer. So for `fdatasync`.
  def fsync
    __check_write_open__
    __flush_gathered__
    IO.__check__(__fd_fsync__(@fileno, false), @path)
    0
  end

  def fdatasync
    __check_write_open__
    __flush_gathered__
    IO.__check__(__fd_fsync__(@fileno, true), @path)
    0
  end

  # CRuby's `rb_io_flush_raw`: writes go at once here, so what is left is what
  # writes in progress have gathered, then `io_unread` of a reader, discarding
  # what a pipe cannot give back.
  def flush
    __check_write_open__
    __flush_gathered__
    __unread__(true)
    self
  end

  def tty?
    __check_open__
    __fs_isatty__(@fileno)
  end
  alias isatty tty?

  def stat
    __check_open__
    __stat__
  end

  def __stat__ = File::Stat.__from__(IO.__check__(__fs_stat__(@fileno, true), @path))

  def binmode
    __check_open__
    @wbinary = true
    __ascii8bit_binmode__
    self
  end

  # What `IO.copy_stream` does to the IO it writes to: of a duplex child, only
  # the write side, as `rb_io_get_write_io` gives it.
  def __write_binmode__
    return __ascii8bit_binmode__ unless __tied__?
    @wbinary = true
    @textmode = false
  end

  # CRuby's `rb_io_ascii8bit_binmode`.
  def __ascii8bit_binmode__
    @binmode = true
    @textmode = false
    @enc = Encoding::BINARY
    @enc2 = nil
    # CRuby drops the converter unfinished, an attribute's quote and all.
    __clear_codeconv__
    # And no conversion options at all: CRuby's `io_ascii8bit_binmode` zeroes
    # the flags and drops the rest, so `xml:` and the error handling go with
    # the newline decorators.
    @ecopts = {}
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

  # CRuby's `rb_io_check_char_readable`, which flushes the write buffer first:
  # here what writes in progress have gathered.
  def __readable__
    __check_open__
    raise IOError, "not opened for reading" unless __reads__?
    __flush_gathered__ if @wgathered && !@wgathered.empty?
  end

  # CRuby's `rb_io_check_writable`. Bytes read ahead are bytes the descriptor
  # has moved past; a write belongs where the reader is.
  def __writable__
    __check_open__
    raise IOError, "not opened for writing" unless __writes__?
    __unread__(true)
  end

  # CRuby's `GetWriteIO` before `rb_io_check_writable`: a write goes to a
  # duplex child's write end, which stays closed once `close` let it go.
  def __write_side__
    __check_write_open__
    __writable__
  end

  def __check_write_open__
    __check_open__
    raise IOError, "closed stream" if @wgone
  end

  # --- encodings ---------------------------------------------------------------
  #
  # CRuby's two slots. `@enc` is the encoding reads answer in, nil for the
  # default external as it is when asked; `@enc2`, when there is one, is the
  # encoding the bytes are in, so reads convert from it to `@enc` and writes
  # back. Without `@enc2` nothing is converted on the way in.

  # `rb_io_ext_int_to_encs`: the slots for an external and an internal
  # encoding asked for, as `[enc, enc2]`. A nil external is the default, a nil
  # internal the default internal, and `:none` (an internal of "-") no
  # conversion.
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
      [default_external && internal != external ? nil : external, nil]
    else
      [internal, external]
    end
  end

  # An IO that reads text in an encoding that is not ASCII-compatible has to
  # be in binmode: CRuby's `validate_enc_binmode`, which is given what is to
  # be assigned, so what it refuses leaves the IO as it was.
  def __validate_enc_binmode__(access, binmode, enc, enc2, ecopts)
    if (access == 0 || access == 2) && enc2.nil? && !binmode && !(enc || Encoding.default_external).ascii_compatible?
      raise ArgumentError, "ASCII incompatible encoding needs binmode"
    end
    if binmode && __newline_decorator__?(ecopts)
      raise ArgumentError, "newline decorator with binary mode"
    end
  end

  def __newline_decorator__?(ecopts = @ecopts)
    ecopts[:universal_newline] || ecopts[:crlf_newline] || ecopts[:cr_newline] || ecopts[:lf_newline] || !ecopts[:newline].nil?
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
    return @enc if __writes__? && !__tied__?
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
    # The options are read after the encodings and only where CRuby's
    # `io_encoding_set` reads them, and a `replace:` refused changes nothing.
    if !second.nil?
      external = __encoding_arg__(first)
      none = !(Encoding === second) && second.to_str == "-"
      internal = __encoding_arg__(second) unless none
      ecopts = __kept_ecopts__(options)
      if none
        enc = external
        enc2 = nil
      else
        enc = internal
        enc2 = internal == external ? nil : external
      end
      # From BINARY there is nothing to convert.
      if enc2 == Encoding::BINARY
        enc = enc2
        enc2 = nil
      end
    elsif first.nil?
      enc, enc2 = __ext_int_to_encs__(nil, nil)
      ecopts = __kept_ecopts__({})
    elsif !(Encoding === first) && (String === first || first.respond_to?(:to_str))
      text = first.to_str
      raise ArgumentError, "invalid name encoding (non ASCII)" unless text.encoding.ascii_compatible?
      # A `bom|` here names the encoding and reads nothing: CRuby reads past a
      # mark only when it opens.
      _, _, _, external, internal, bom = IO.__parse_mode__("r:#{text}")
      ecopts = __kept_ecopts__(options)
      enc, enc2 = __ext_int_to_encs__(external, internal, bom)
    else
      enc, enc2 = __ext_int_to_encs__(__encoding_arg__(first), nil)
      ecopts = __kept_ecopts__({})
    end
    __encoding_set__(enc, enc2, ecopts)
    self
  end

  # The end of CRuby's `io_encoding_set`: nothing is assigned until the check
  # has passed.
  def __encoding_set__(enc, enc2, ecopts)
    __validate_enc_binmode__(@access, @binmode, enc, enc2, ecopts)
    @enc = enc
    @enc2 = enc2
    @ecopts = ecopts
    # From here `@ecopts` says, but a duplex child's write side is not this.
    @textmode = false unless __tied__?
    # CRuby drops the converter unfinished: an attribute's quote open is
    # never closed, and the next write opens another. The read converter
    # goes too, and the characters it held count until a read of characters
    # makes another.
    __clear_codeconv__
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
    __byte_readable__
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

  # Read past a byte order mark and take the encoding it names, as the
  # external, keeping the internal: CRuby's `io_set_encoding_by_bom`. Answers
  # the encoding, or nil when there is no mark, which leaves the internal
  # encoding as what the IO reads in, without conversion. An IO that does not
  # read has no mark.
  def __bom__
    if __reads__?
      nil while @rbuf.bytesize < 4 && __fill__
      BOMS.each do |mark, name|
        next unless @rbuf.byteslice(0, mark.bytesize) == mark
        __take__(mark.bytesize)
        external = Encoding.find(name)
        if @enc2.nil?
          enc, enc2 = __ext_int_to_encs__(external, nil)
        else
          enc = @enc
          enc2 = enc == external ? nil : external
        end
        __encoding_set__(enc, enc2, __kept_ecopts__({}))
        return external
      end
    end
    @enc2 = nil
    nil
  end

  # Bytes read, in the IO's encoding and then in the one reads answer in,
  # with the read converter's `xml:` decorator. That converter is the
  # stream's, as CRuby's `make_readconv` keeps one for the IO, so an
  # attribute's quote opens with the first character it makes and closes
  # at the end (#466, #468): each piece is escaped without its quotes. `taken`
  # is what came off the buffer, where that is not `bytes` themselves.
  def __decode__(bytes, taken = bytes, universal = false)
    return __convert_in__(bytes) || __in_read__("") if __stream_in__?
    bytes.force_encoding(__encoding__)
    xml = __xml_in__
    if @enc2.nil?
      return bytes if xml.nil?
      out = String.__escape_xml__(bytes, xml)
      out = out.byteslice(1, out.bytesize - 2).force_encoding(bytes.encoding) if xml == :attr
    else
      begin
        options = __read_options__(@ecopts)
        options[:universal_newline] = true if universal
        out = bytes.encode(__read_encoding__, **options)
      rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError => error
        __give_back_past_error__(taken)
        raise error
      end
      out = out[1...-1] if xml == :attr
    end
    return out unless xml == :attr && !@rquote && !out.empty?
    @rquote = true
    (__in_read__("\"").b << out.b).force_encoding(out.encoding)
  end

  # What CRuby's `fill_cbuf` leaves when a conversion raises, for a read that
  # took all of `bytes` before `String#encode` raised: a converter finds the
  # error again, and what it did not get to goes back to be read next, as do
  # those it read past the error's. That read has not ended, whatever it saw
  # of the bytes, so it still made a converter. It goes as `more_char` does, a
  # call at a time: what the calls before the error made went with the read,
  # an attribute's quote among it, and what the one that raised made stays
  # held, as in `cbuf`, which only a `read` takes with it. With universal
  # newline the converter has its decorator, as `__make_readconv__` gives it,
  # and a CR it holds before the error, to tell from a CRLF, goes back too:
  # CRuby's keeps it, and the next read makes it the `"\n"` (#555). Measured.
  def __give_back_past_error__(bytes)
    options = __read_ecopts__
    options[:universal_newline] = true if __universal_newline__?
    converter = __converter__(__encoding__, __read_encoding__, options)
    converter.primitive_convert("a".encode(__encoding__), "".b, nil, nil, Encoding::Converter::PARTIAL_INPUT) if @rquote
    made = nil
    loop do
      made = "".b
      result = converter.primitive_convert(bytes, made, nil, nil, Encoding::Converter::AFTER_OUTPUT)
      @rquote = true if __xml_in__ == :attr && !made.empty?
      break unless result == :after_output
    end
    @rbuf = converter.putback.b + bytes.b + @rbuf
    if options[:universal_newline] && converter.finish.b.start_with?("\n".encode(__read_encoding__).b)
      @rpend = "\r".encode(__encoding__).b
    end
    __hold_chars__(made)
  end

  # Whether the IO has a read converter, as CRuby's `NEED_READCONV`: an
  # internal encoding, or a newline option.
  def __read_converter__?
    !@enc2.nil? || __newline_decorator__?
  end

  # The `xml:` decorator a read converter has, or nil: without one, reads
  # are not decorated. Measured.
  def __xml_in__
    xml = @ecopts[:xml]
    (xml == :text || xml == :attr) && __read_converter__? ? xml : nil
  end

  # Whether reads go through the IO's own `Encoding::Converter`, `@readconv`,
  # because what a conversion knows carries from one read to the next:
  # ISO-2022-JP's G0, the byte order a BOM'd UTF-16 or UTF-32 read, the mark
  # one writes first (#420). `String#encode` starts and finishes each time.
  # So do they when `reopen` left a converter of other encodings than the IO
  # has now, which `String#encode` could not stand in for (#517), and where
  # `__newline_on_bytes__?`, which it could not either, and once a decorator
  # moved what came before a separator past it (#566).
  def __stream_in__?
    return __read_converter__? if @rkept || @rstream
    return __newline_on_bytes__? if @enc2.nil?
    __stateful__?(@enc2) || __stateful__?(__read_encoding__)
  end

  # Whether universal newline has only bytes to go on: `BOM|` that finds no
  # mark clears the internal encoding and leaves an encoding that is not
  # ASCII-compatible with no conversion, which CRuby's `validate_enc_binmode`
  # is not run again to refuse. CRuby's converter is then the decorator alone,
  # so a CR in the bytes is a CR, and the characters are cut from what it
  # makes (#525).
  def __newline_on_bytes__?
    @enc2.nil? && !@enc.nil? && !@enc.ascii_compatible? && __universal_newline__?
  end

  def __stateful__?(encoding)
    encoding.dummy? && String.__supported_target__(encoding)
  end

  # CRuby's `make_readconv`, with its `xml:` decorators and universal newline
  # in the one converter, where a read that starts and finishes a conversion
  # has them done around it. Without an internal encoding it is of the
  # decorators alone, `""` to `""`, which `NewlineBytes` stands in for where
  # the IO reads through it itself (#550).
  def __make_readconv__
    __readconv__
    source, target, ecopts = @rmade
    return NewlineBytes.new(__read_encoding__, ecopts[:xml]) if source.nil? && !@rkept
    options = __read_options__(ecopts)
    options[:universal_newline] = true if __universal_newline__?(ecopts)
    return Encoding::Converter.new("", "", **options) if source.nil?
    __converter__(source, target, options)
  end

  # CRuby's `rb_io_reopen` and `io_reopen` replace the encodings and leave
  # `readconv`, and `make_readconv` makes none while there is one, so reads
  # with a converter go on through the one a read already made, its options
  # and all, and what it writes is called what the IO reads in now. Reads
  # that have no converter leave it there for a later `reopen` to come back
  # to. It carries on where the old one was: an attribute's quote is open, a
  # CR or the first byte of a character is inside, which `@rpend` holds for
  # the next read that reaches it, and one that has finished reads nothing.
  # Whatever drops the converter ends this. Measured.
  # One of decorators alone is kept too, a `NewlineBytes` as it is, so what
  # it writes is the bytes, called what the IO reads in now (#550).
  def __keep_readconv__
    made = @rmade
    return if made.nil? || @rkept || made == __read_made__
    @rkept = true
    return unless @readconv.nil?
    begin
      @readconv = __make_readconv__
    rescue Encoding::ConverterNotFoundError
      # The read that asked for it raised this too, and so made none.
      @rmade = nil
      @rkept = false
      return
    end
    @readconv.primitive_convert(made[0] ? "a".encode(made[0]) : "a".b, "".b, nil, nil, Encoding::Converter::PARTIAL_INPUT) if @rquote
    @readconv.finish if @rfinished
  end

  # A converter the IO keeps. One whose `replace:` has no spelling in what it
  # writes is not found, as CRuby's `rb_econv_open_opts` gives up. Measured.
  def __converter__(source, target, options)
    Encoding::Converter.new(source, target, **options)
  rescue Encoding::UndefinedConversionError
    raise Encoding::ConverterNotFoundError, "code converter not found (#{source.name} to #{target.name})"
  end

  # CRuby's converter of the universal newline decorator alone, as much of it
  # as `__newline_on_bytes__?` reads through and `reopen` keeps: it makes
  # bytes, whatever they are called, and holds a CR for the byte after it. An
  # attribute's quote comes first and opens on the first byte, whatever that
  # makes.
  # ponytail: only that read, which it does for less than
  # `Encoding::Converter.new("", "", ...)`, as each call of that costs as much
  # as one of any other converter, twice what a `getc` costs here. The upgrade
  # is a cheaper call, which `__next_chars__` names too.
  class NewlineBytes
    def initialize(encoding, xml)
      @encoding = encoding
      @xml = xml
      @cr = false
      @finished = false
    end

    def primitive_convert(source, out, _offset, _room, flags)
      return :finished if @finished
      seen = !source.empty?
      made = "".b
      if flags & Encoding::Converter::AFTER_OUTPUT == 0
        made, @cr = String.__newline_turn__(source, "universal_newline", @cr)
        source.clear
      else
        used = 0
        while made.empty? && used < source.bytesize
          made, @cr = String.__newline_turn__(source.byteslice(used, 1), "universal_newline", @cr)
          used += 1
          break if @xml == :attr && !@quoted
        end
        source.replace(source.byteslice(used, source.bytesize - used))
      end
      made = String.__escape_xml__(made, @xml).b if @xml
      if @xml == :attr
        made = made.byteslice(1, made.bytesize - 2)
        if seen && !@quoted
          made = "\"".b + made
          @quoted = true
        end
      end
      out.force_encoding(Encoding::BINARY) << made
      out.force_encoding(@encoding)
      return :after_output unless source.empty?
      :source_buffer_empty
    end

    def finish
      held = @cr ? "\n" : ""
      held = String.__escape_xml__(held, @xml)[1...-1] if @xml == :attr
      held = (@quoted ? "" : "\"") + held + "\"" if @xml == :attr
      @cr = false
      @finished = true
      held.b.force_encoding(@encoding)
    end

    # It makes as much as it reads, so it never stops short (#611).
    def __goes_on__=(_goes_on); end
    def __suspended__? = false
  end
  private_constant :NewlineBytes

  # What the read converter makes of `bytes`, going on from the last it read:
  # partial input, so what they end part way through waits inside. Nil from a
  # converter that has finished, which leaves them for the one after it.
  def __convert_in__(bytes)
    __convert_onto__(bytes, String.new(encoding: __read_encoding__), nil, Encoding::Converter::PARTIAL_INPUT)
  end

  # `__convert_in__` onto `out`, up to `room` bytes of it. The bytes it did not
  # get to go back to be read next, and on an error so do those it read past
  # the error's, as CRuby's `fill_cbuf` gives them back. What a call stopping
  # after output made as well as the error, a BOM or an attribute's quote,
  # stays held, in the buffer `fill_cbuf` was filling: only a read that had
  # taken what it made before loses that. A converter that has finished reads
  # nothing and answers nil, as `fill_cbuf`'s MORE_CHAR_FINISHED: the bytes
  # wait in the buffer for the converter made after it. Measured. With
  # `goes_on`, a conversion that never ends stops short each call and the
  # next goes on with it (#611).
  def __convert_onto__(bytes, out, room, flags, goes_on = false)
    @readconv ||= __make_readconv__
    if goes_on
      @readconv.__goes_on__ = true
      begin
        result = @readconv.primitive_convert(bytes, out, nil, room, flags)
      ensure
        @readconv.__goes_on__ = false
      end
    else
      result = @readconv.primitive_convert(bytes, out, nil, room, flags)
    end
    out.force_encoding(__read_encoding__) if @rkept
    return out if result == :source_buffer_empty
    if result == :after_output || result == :destination_buffer_full || result == :finished
      @rbuf = bytes.b + @rbuf
      return result == :finished && out.empty? ? nil : out
    end
    @rbuf = @readconv.putback.b + bytes.b + @rbuf
    __hold_chars__(out.b) if flags & Encoding::Converter::AFTER_OUTPUT != 0
    raise @readconv.last_error
  end

  # CRuby's `rbuf` and `cbuf` for an IO with a read converter: what a fill of
  # the buffer asks for where reads go through it with options, so what one
  # call of it is given, and what it can make. Measured.
  READ_CONVERTER_BYTES = 131072
  private_constant :READ_CONVERTER_BYTES

  # How many bytes `__next_chars__` offers a converter with no options, enough
  # for an escape sequence and the character after it, or a BOM and a UTF-32
  # character. One longer goes round again; what is not read comes back.
  NEXT_CHARS_BYTES = 8
  private_constant :NEXT_CHARS_BYTES

  # What the read converter makes next, or nil at the end of the bytes. It
  # stops after output, as CRuby's `fill_cbuf` does, so an escape sequence is
  # read with the character after it, and `pos` is past that and no further.
  # A converter of more than one transcoder can stop with nothing out yet, as
  # a CR universal newline holds. With options it is given all that is
  # buffered, as CRuby's is, since how far a call goes hangs on it: only with
  # input left does output stop the call, and references for characters it
  # cannot write do not stop it at all. With none, a character's output stops
  # it wherever the input ends. Measured. `NewlineBytes` stops after output
  # whatever it is given, so it is given a byte.
  # ponytail: all that is buffered is copied per character, 40% to 80% of a
  # `getc` with options on a long file. The upgrade is the cheaper call below.
  # ponytail: a call of the converter for each character, which is most of
  # what a `getc` costs here. The upgrade is a cheaper call.
  # With `goes_on`, for `gets` with a limit, a conversion that never ends but
  # at its room, as `xml:` after a broken sequence (#543), goes on from where
  # its last call stopped short of that, with nothing left to read, as
  # CRuby's `fill_cbuf` does after MORE_CHAR_SUSPENDED; so the limit takes
  # that much of what it makes, and the next read goes on with it (#611).
  # Without it the read ends there, where CRuby's never does.
  def __next_chars__(goes_on = false)
    __unpend__ if @rkept
    return nil if @rfinished
    flags = Encoding::Converter::PARTIAL_INPUT | Encoding::Converter::AFTER_OUTPUT
    out = String.new(encoding: __read_encoding__)
    while !@rbuf.empty? || goes_on && @readconv&.__suspended__? || __fill__
      plain = __plain_readconv__?
      count = NewlineBytes === @readconv ? 1 : (plain ? NEXT_CHARS_BYTES : @rbuf.bytesize)
      return nil unless __convert_onto__(__take__(count), out, plain ? nil : READ_CONVERTER_BYTES, flags, goes_on)
      next if out.empty?
      @rnewline = out.getbyte(-1) == 0x0A
      return out
    end
    nil
  end

  # Whether `__chars_to_line_end__` can go by the bytes read: a separator
  # ending in an ASCII character has that character's byte in them, and
  # nothing but universal newline makes one from something else, or a
  # replacement or decorator more than it read, and by the bytes where
  # `__newline_on_bytes__?` makes them whatever they are called. Not through a
  # converter `reopen` left, whose bytes are not those of the encodings the IO
  # has.
  # ponytail: any other line stays a character at a time at `__next_chars__`'s
  # cost; upgrade with the cheaper call that names.
  def __line_runs__?(sep, enc)
    return false if @rkept
    return false unless enc.ascii_compatible? ? sep.nil? || sep.dup.force_encoding(enc)[-1].ascii_only? : __newline_on_bytes__?
    @ecopts.empty? || (@ecopts.size == 1 && __universal_newline__?)
  end

  # `__next_chars__` for a line, which `__line_runs__?` lets go faster: what
  # comes before the next byte that can end it cannot, so those bytes go to
  # the converter in one call, and `pos` stops where `__next_chars__` would.
  # That byte goes with them in ISO-2022-JP, where it is the whole character;
  # in UTF-16 or UTF-32 it is a character at a time from there, as the bytes
  # after it can be what makes the character, and likewise from a CR universal
  # newline holds. `__newline_on_bytes__?` makes a character of each byte, so
  # that byte goes with them there too, a CRLF's LF with its CR, and only a CR
  # that could be held waits. A newline can leave the next CR held, and a held
  # CR comes out with the next character, a line of its own that an error
  # later in a run would lose, so after one it is a character at a time to the
  # next.
  # With `room` left of a limit, no more bytes go than cannot fill it: four a
  # byte, more than a character's bytes can make, the partial one the
  # converter holds among them. Answers "" where the bytes made nothing yet,
  # and nil where the converter has finished.
  def __chars_to_line_end__(sep, room)
    return __next_chars__ if @rnewline
    return nil if @rfinished || @rbuf.empty? && !__fill__
    bytes = __newline_on_bytes__?
    at = sep && @rbuf.index(sep[-1])
    at += 1 if at && (bytes || __encoding__ == Encoding::ISO_2022_JP)
    if sep&.end_with?("\n") && __universal_newline__?
      cr = @rbuf.index("\r")
      if cr && (at.nil? || cr < at)
        at = cr
        at += 2 if bytes && @rbuf.getbyte(cr + 1) == 10
      end
    end
    count = at || @rbuf.bytesize
    count = [count, room / 4].min if room && room > 0
    count -= 1 if bytes && count > 0 && @rbuf.getbyte(count - 1) == 13
    count == 0 ? __next_chars__ : __convert_in__(__take__(count))
  end

  # An ASCII String in the encoding reads answer in.
  def __in_read__(text)
    enc = __read_encoding__
    enc.ascii_compatible? ? text.b.force_encoding(enc) : text.encode(enc)
  end

  # What the read converter makes when a read of characters reaches the
  # end: an attribute's closing quote, or `""` when no character opened it.
  # A converter that has finished makes nothing more, and CRuby's next read of
  # characters then finds nothing, drops it and stops, whatever bytes came in
  # between; the read after makes one again, with another `""` (#544).
  # So a read that goes on until the converter stops, as `read` and `gets`
  # without a limit do, has `""` every time and `each_line` never ends,
  # while one a limit stops first leaves it finished for the next to find
  # nothing. Measured.
  def __finish_in__
    return __finish_readconv__ if __stream_in__?
    unless __xml_in__ == :attr
      @rfinished = false
      __read_ended__
      return nil
    end
    if @rfinished
      @rfinished = false
      @rquote = nil
      @rmade = nil
      return nil
    end
    quote = @rquote ? "\"" : "\"\""
    @rquote = false
    @rfinished = true
    __in_read__(quote)
  end

  # Whether the read converter has finished, for the next read of characters
  # to find nothing. One a `reopen` with no converter left waits for a later
  # `reopen` that has one, and reads go past it (#551). Measured.
  def __finished_in__? = @rfinished && __read_converter__?

  # A read of characters that ran the read converter to the end of the bytes,
  # which finished it (#551). One of `xml: :attr` makes the attribute's
  # quotes as it finishes, which are held, as in CRuby's `cbuf`. Measured.
  def __finished_at_end__
    return if @rfinished
    if __xml_in__ == :attr
      @cbuf = (@cbuf || "".b) << __finish_in__.b
    else
      @rfinished = true
    end
  end

  # A read with a converter that reached the end, where CRuby's drops it. A
  # line through `__converted_line__` goes on to `__finish_in__` for that, and
  # one read `whole` by bytes says so itself: a limit can stop the other first.
  def __read_ended__
    @rmade = nil if __read_converter__?
  end

  # `__finish_in__` where the IO keeps a converter, which writes what it has
  # left, an attribute's closing quote or the escape back to ASCII among it,
  # and stays, finished, until the next read of characters finds nothing,
  # answers nil and drops it, as CRuby's `fill_cbuf` answers
  # MORE_CHAR_FINISHED; bytes that arrive in between wait for the read after,
  # which makes another (#544). One that wrote nothing goes at once. One that
  # raises stays, as CRuby's `fill_cbuf` keeps it, for the next read to have
  # what it has left, a CR it held as a newline among it. Measured.
  def __finish_readconv__
    if @rfinished
      @rfinished = false
      @readconv = nil
      __forget_readconv__
      return nil
    end
    @readconv ||= __make_readconv__
    out = @readconv.finish
    if out.empty?
      @readconv = nil
      __forget_readconv__
      return nil
    end
    @rfinished = true
    out
  end

  # Characters the read converter has made and not handed out, in the
  # encoding reads answer in: what `ungetc` gave back, the rest of an
  # entity `getc` handed out the first character of, and the character
  # read past a lone CR to tell it from a CRLF (#468, #470). CRuby's
  # `cbuf`; `pos` does not count them, as they are read.
  def __chars_held__?
    !@cbuf.nil? && !@cbuf.empty?
  end

  # Whether a read of characters takes what is held. `reopen` can leave
  # characters held on an IO with no read converter: those read past them,
  # and a read of bytes still refuses, until a mode with a converter hands
  # them out, or makes a converter that starts without them where
  # `__drop_readconv__` dropped the last. Measured.
  def __chars_read__?
    __chars_held__? && __read_converter__?
  end

  # The characters held, taken, as bytes.
  def __take_chars__
    held = @cbuf || "".b
    @cbuf = "".b
    held.b
  end

  # Puts characters back before those held.
  def __hold_chars__(text)
    return if text.empty?
    @cbuf = text.b << (@cbuf || "".b)
  end

  # The first character held, in the encoding reads answer in.
  def __take_char__
    held = @cbuf.force_encoding(__read_encoding__)
    char = held[0]
    @cbuf = held.byteslice(char.bytesize, held.bytesize - char.bytesize).b
    char
  end

  # CRuby's `io_getc` through a converter `reopen` left or a `NewlineBytes`,
  # whose bytes need not end where a character of what they are called does:
  # it makes more until the first is whole. At the end what is cut short goes a byte at a time,
  # and the converter stays finished until the last has. Measured.
  def __kept_char__
    enc = __read_encoding__
    while !__chars_held__? || __held_cut_short__?(@cbuf, enc)
      made = __next_chars__
      readconv = @readconv
      kept = @rmade
      made ||= __finish_in__
      unless made.nil?
        @cbuf = (@cbuf || "".b) << made.b
        next
      end
      return nil unless __chars_held__?
      unit = @cbuf.byteslice(0, 1).force_encoding(enc)
      @cbuf = @cbuf.byteslice(1, @cbuf.bytesize - 1).b
      @readconv, @rmade, @rkept, @rfinished = readconv, kept, true, true if __chars_held__?
      return unit
    end
    __take_char__
  end

  # CRuby's NEEDMORE: whether `bytes` start with a character of `enc` that more
  # bytes would finish.
  # ponytail: CESU-8 answers no, so its tail goes as broken characters; upgrade
  # by asking as UTF-8 is asked.
  def __held_cut_short__?(bytes, enc)
    head = bytes.byteslice(0, 4).force_encoding(enc)
    size = head.bytesize
    case enc
    when Encoding::UTF_8
      first = head.getbyte(0)
      return false if first < 0xC2 || first > 0xF4
      room = (first >= 0xF0 ? 4 : (first >= 0xE0 ? 3 : 2)) - size
      room > 0 && [0x80, 0x90, 0xA0].any? { |byte| (head.b + byte.chr * room).force_encoding(enc).valid_encoding? }
    when Encoding::UTF_16LE, Encoding::UTF_16BE
      return true if size < 2
      little = enc == Encoding::UTF_16LE
      return false unless head.getbyte(little ? 1 : 0) & 0xFC == 0xD8
      size == 2 || size == 3 && (little || head.getbyte(2) & 0xFC == 0xDC)
    when Encoding::UTF_32LE, Encoding::UTF_32BE then size < 4
    else head.__cut_short__
    end
  end

  # CRuby's `clear_codeconv`, both converters.
  def __clear_codeconv__
    __clear_writeconv__
    __drop_readconv__
  end

  # CRuby's `clear_readconv`: the converter goes, and an attribute's quote
  # with it, but the characters it held still count, so a read of bytes
  # refuses and `eof?` is false, until a read of characters makes a converter
  # again, which starts without them. Measured.
  def __drop_readconv__
    @readconv = nil
    @rpend = nil
    @rfinished = false
    @readconv_dropped = __chars_held__?
    __forget_readconv__
  end

  # No read has made a converter, so the next makes one of the IO's encodings.
  def __forget_readconv__
    @rmade = nil
    @rkept = false
    @rstream = false
    @rquote = nil
  end

  # A read of bytes, where the read converter holds characters: CRuby's
  # `rb_io_check_byte_readable`. Measured.
  def __byte_readable__
    raise IOError, "byte oriented read for character buffered IO" if __chars_held__?
  end

  # What CRuby's converter reads past a lone CR to tell it from a CRLF, so
  # `pos` counts it (#470, #472). A whole character is held, converted. A
  # CR the converter keeps itself, to tell in turn: it is the bytes' and
  # not a character held, so a read of bytes goes past it, and the next
  # read of characters starts with it, one "\n" with a LF after it and
  # "\n" before anything else. The first byte of a longer character is
  # held as it is without an internal encoding, the converter there being
  # the newline decorator alone. With one, CRuby's decorator comes after a
  # transcoder, which reads the whole character, so that is held, converted
  # (#551). From UTF-8 into UTF-16 or UTF-32 `rb_econv_decorate_at_last`
  # puts the decorator first, so the converter keeps the one byte it reads,
  # as it keeps the first of bytes that make no character. Nothing after the
  # CR is the end, which CRuby's converter looked for, and so finished
  # (#551). `pos` or `seek` over bytes buffered drops what the converter
  # kept. Bytes that make no character raise on the read the CR answers,
  # the transcoder having read them with it, and the CR goes back as from
  # any read that raises (#555): before a decorator that comes first, only
  # a byte no UTF-8 character starts with does. Measured (#595).
  # ponytail: in UTF-16 or UTF-32 the first unit of a surrogate pair stays
  # in the buffer, `pos` a unit short; upgrade by keeping it as the first
  # byte of a longer character is.
  def __read_ahead__(cr, unit)
    if @rbuf.bytesize < unit
      __finished_at_end__ if @rbuf.empty?
      return
    end
    head = @rbuf.byteslice(0, unit)
    if head == cr
      @rpend = __take__(unit)
      return
    end
    case unit
    when 1
      enc = __encoding__
      unless head.dup.force_encoding(enc).valid_encoding?
        if @enc2.nil?
          @cbuf = (@cbuf || "".b) << __take__(1)
          return
        end
        if enc == Encoding::UTF_8 && !__read_encoding__.ascii_compatible?
          lead = head.getbyte(0)
          return __decode__(cr + __take__(1)) if (lead < 0xC2 || lead > 0xF4) && __raises_past_cr__?
        else
          nil while __char_cut_short__ && __fill__(1)
          unit = __char_length__
          unit = @rbuf.bytesize if unit > @rbuf.bytesize
          unless @rbuf.byteslice(0, unit).force_encoding(enc).valid_encoding?
            # What the error names reads past the character, as on the stream.
            return __decode__(cr + __take__([@rbuf.bytesize, 4].min)) if __raises_past_cr__?
            unit = 1
          end
        end
        if unit == 1
          return __replaced_past_cr__(cr) if __replaces_past_cr__?
          @rpend = __take__(1)
          return
        end
      end
      if __replaces_past_cr__? && __undefined_in__?(@rbuf.byteslice(0, unit))
        out = __decode__(__take__(unit))
        @rpend = cr
        return out
      end
    when 2
      high = __encoding__ == Encoding::UTF_16LE ? head.getbyte(1) : head.getbyte(0)
      return if high & 0xF8 == 0xD8
    end
    @cbuf = (@cbuf || "".b) << __decode__(__take__(unit)).b
    nil
  end

  # Whether the character past a lone CR is replaced on the read the CR
  # answers: CRuby's transcoder, which comes before the decorator, reads it
  # there, and its replacement comes out ahead of the decorator, which still
  # holds the CR, so the `"\n"` comes after it, `pos` past it, and a LF
  # after it makes a CRLF of the CR (#626). Measured. From UTF-8 into UTF-16
  # or UTF-32 the decorator comes first, and so does the `"\n"`.
  def __replaces_past_cr__?
    !@enc2.nil? && !__stream_in__? && (__encoding__ != Encoding::UTF_8 || __read_encoding__.ascii_compatible?)
  end

  # The replacement for bytes past a lone CR that make no character. The
  # CR is held again, `@rpend`, and so is what the transcoder read past
  # them to tell, which is its own, so `pos` is past both and the next read
  # starts with them (#626). Measured.
  def __replaced_past_cr__(cr)
    window = @rbuf.byteslice(0, [@rbuf.bytesize, 4].min).force_encoding(__encoding__)
    broken = 1
    again = 0
    begin
      window.encode(__read_encoding__)
    rescue Encoding::InvalidByteSequenceError => error
      broken = error.error_bytes.bytesize
      again = error.readagain_bytes.to_s.bytesize
    end
    out = __decode__(__take__(broken))
    @rpend = cr + __take__(again)
    out
  end

  # Whether a whole character has no spelling in the encoding reads answer
  # in, and `undef: :replace` replaces it.
  def __undefined_in__?(bytes)
    return false unless @ecopts[:undef] == :replace
    bytes.dup.force_encoding(__encoding__).encode(__read_encoding__)
    false
  rescue Encoding::UndefinedConversionError
    true
  end

  # A line that a lone CR ends, where `__read_ahead__` replaced what came
  # past it: the replacement comes before the `"\n"`, and the CR it held
  # again is told from a CRLF as before, which a line reads on to (#626).
  def __before_newline__(text, before, cr, unit)
    return text if before.nil?
    nl = "\n".encode(text.encoding).b
    text = text.b
    text = text.byteslice(0, text.bytesize - nl.bytesize) if text.end_with?(nl)
    lf = "\n".encode(__encoding__).b
    loop do
      text << before.b
      __unpend__
      __take__(cr.bytesize)
      nil while @rbuf.bytesize < lf.bytesize && __fill__(lf.bytesize - @rbuf.bytesize)
      if @rbuf.byteslice(0, lf.bytesize) == lf
        __take__(lf.bytesize)
        break
      end
      before = __read_ahead__(cr, unit)
      break if before.nil?
    end
    (text << nl).force_encoding(__read_encoding__)
  end

  # Whether bytes past a lone CR that make no character raise on that read:
  # a replacement is made there, `__replaced_past_cr__` (#626), and a read
  # through `@readconv` has the converter read past the CR itself.
  def __raises_past_cr__? = @ecopts[:invalid] != :replace && !__stream_in__?

  # The bytes the converter kept past a lone CR go back before the buffer,
  # for a read of characters to convert them first (#472). A read that does
  # not go through the converter `reopen` left passes them by, as it does it.
  def __unpend__
    return if @rpend.nil? || @rkept && !__read_converter__?
    @rbuf = @rpend + @rbuf
    @rpend = nil
  end

  # The options a read transcodes with: CRuby's `make_readconv` leaves out
  # the decorators for writing. Universal newline is done on the way in.
  def __read_ecopts__
    __readconv__
    __read_options__(@ecopts)
  end

  def __read_options__(ecopts)
    ecopts.reject { |key, _| %i[newline crlf_newline cr_newline lf_newline].include?(key) }
  end

  # Whether the converter reads go through has no options.
  def __plain_readconv__? = (@rkept ? @rmade[2] : @ecopts).empty?

  # What a converter made now would convert: without an internal encoding,
  # CRuby's is the decorators alone.
  def __read_made__
    @enc2.nil? ? [nil, nil, @ecopts] : [__encoding__, __read_encoding__, @ecopts]
  end

  # CRuby's `make_readconv`, which a read that converts makes before it reads
  # anything, at the end as well: an IO has a read converter when it has an
  # internal encoding or a newline option. With both `xml:` decorators, the
  # quote's too, it asks for all of them in one converter, and none has them
  # (#458, #461). Text mode gives it universal newline (#465). What it would
  # convert is noted, for `reopen`, which leaves it, to make it from: reads
  # that need nothing kept from one to the next have no converter to leave
  # (#517). Measured.
  def __readconv__
    if @readconv_dropped && __read_converter__?
      @cbuf = "".b
      @readconv_dropped = false
    end
    # An IO with neither has no converter, and is most of what reads here.
    return unless @rmade.nil? && (@enc2 || !@ecopts.empty?) && __read_converter__?
    unless @ecopts[:xml].equal?(XML_BOTH)
      @rmade = __read_made__
      return
    end
    newlines = __universal_newline__? ? ["universal_newline"] : []
    raise String.__decorators_not_found__(@enc2 || __encoding__, __read_encoding__, newlines + %w[xml_text xml_attr_content xml_attr_quote], nil)
  end

  def __universal_newline__?(ecopts = @ecopts)
    ecopts[:universal_newline] || ecopts[:newline] == :universal
  end

  # A String about to be written, in the encoding the IO writes: `@enc2`, or
  # `@enc` unless that is BINARY. CRuby's `do_writeconv`, with the newline
  # decorators run by `String#encode`. Measured.
  #
  # `xml: :attr`'s quote is the stream's, not each write's: CRuby's
  # `make_writeconv` keeps that stateful decorator in the IO's own converter,
  # made by the first write of a String that is not empty or of any number
  # other than one, which opens the quote with the first character it is given
  # and closes it when the IO closes. So a write is escaped without its quotes,
  # and the first with anything in it opens one. `@xml_quote` is nil before
  # there is a converter, then whether the quote is open.
  #
  # What the first write that needs a conversion finds stays: the options in
  # `@wpre`, and in `@wtarget` the encoding, unless that is `:current`, which
  # goes on following the IO's. `reopen` leaves them, whatever it takes, but
  # for the newlines, which a binary IO it took with an IO drops. They go with
  # the converter, as bytes buffered given back take it too. Measured.
  def __encode_out__(text)
    if __tied__?
      __refuse_for_text_mode__(text) if @textmode && !text.encoding.ascii_compatible?
      return text
    end
    return text unless __need_writeconv__?
    __make_writeconv__
    options = @wpre
    target = @wtarget
    if target.equal?(:current)
      target = __write_target__
      return text if target.nil?
    end
    newline = String.__newline_decorators__(options).first
    ascii = @writeconv&.source_encoding
    xml = options[:xml]
    if (xml == :attr || xml.equal?(XML_BOTH)) && @xml_quote.nil?
      __xml_quote__(false)
      IO.__unfinished__[self] = true
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
      if !text.encoding.ascii_compatible? && (@textmode || %i[newline universal_newline crlf_newline cr_newline lf_newline].any? { |key| @ecopts[key] })
        __refuse_for_text_mode__(text)
      end
      return text if newline.nil? && xml.nil?
      decorators = options.select { |key, _| %i[newline crlf_newline cr_newline lf_newline xml].include?(key) }
      return __open_quote__(text.b.encode(Encoding::BINARY, **decorators), xml).force_encoding(text.encoding)
    end
    if ascii
      # Into an encoding that is not ASCII-compatible, CRuby converts twice,
      # even from that same encoding with no decorator, so an invalid String
      # raises: with the decorators to the ASCII-compatible encoding it pairs
      # with, then on through the IO's own converter, which keeps ISO-2022-JP's
      # G0 from one write to the next, and a character a write ends part way
      # through (#420).
      # The quote goes in before the second, so one that raises leaves it
      # open but unwritten, as CRuby's converter does. Measured.
      return @writeconv.convert(__open_quote__(text.encode(ascii, **options), xml))
    end
    # In its own encoding a String is still decorated: CRuby's `encode`
    # runs the decorators over its bytes, invalid ones and all.
    return text if newline.nil? && xml.nil? && text.encoding == target && options[:invalid] != :replace
    __open_quote__(text.encode(target, **options), xml)
  end

  def __refuse_for_text_mode__(text)
    raise ArgumentError, "ASCII incompatible string written for text mode IO without encoding conversion: #{text.encoding}"
  end

  # The encoding the IO writes: `@enc2`, or `@enc` unless that is BINARY.
  def __write_target__ = @enc2 || (@enc == Encoding::BINARY ? nil : @enc)

  # CRuby's `NEED_WRITECONV`.
  def __need_writeconv__?
    !(@enc.nil? || @enc == Encoding::BINARY) || @textmode || __newline_decorator__? || !@ecopts[:xml].nil?
  end

  # CRuby's `make_writeconv`. For an encoding that is not ASCII-compatible it
  # makes the converter into it from the ASCII-compatible encoding it pairs
  # with, with only the error handling. Done once, by the first write of a
  # String that is not empty or of any number other than one: `reopen` keeps it
  # all, and writes go on through it, as CRuby's do, whatever it opened. Without
  # an encoding, or with a converter to go on through, the encoding stays as
  # well. Measured.
  def __make_writeconv__
    return if @wpre
    options = __write_ecopts__
    target = __write_target__
    ascii = target && Encoding::Converter.asciicompat_encoding(target)
    unless ascii.nil?
      @writeconv = __converter__(ascii, target, options.slice(:invalid, :undef, :replace))
      @finalizer&.writeconv = @writeconv
      IO.__unfinished__[self] = true
    end
    stateful = options[:xml] == :attr || options[:xml].equal?(XML_BOTH)
    @wtarget = target.nil? || ascii || stateful ? target : :current
    @wpre = options
  end

  # What `encode` with `xml: :attr` made, in an ASCII-compatible encoding,
  # as the stream's quote has it: without the quotes, and with the opening
  # one if this is the first character.
  def __open_quote__(quoted, xml)
    return quoted unless xml == :attr
    content = quoted.byteslice(1, quoted.bytesize - 2)
    return content if content.empty? || @xml_quote
    __xml_quote__(true)
    "\"".b.force_encoding(content.encoding) << content
  end

  # What CRuby's `finish_writeconv` writes when the IO closes: the closing
  # quote, or `""` if no character opened it, and the converter's end, the
  # escape back to ASCII. An error raises having written nothing more: a
  # character a write began and none finished. What writes in progress have
  # gathered goes first, as CRuby flushes its write buffer. Measured.
  def __finish_writeconv__
    __flush_gathered__
    quote = __closing_quote__(@xml_quote)
    converter = @writeconv
    __clear_writeconv__
    return if quote.nil? && converter.nil?
    closing = IO.__closing__(quote, converter)
    __write_all__(closing) unless closing.empty?
  end

  # Each write in progress, oldest first, and emptied, as it may yet write the
  # rest. A tty has no write buffer: the list goes, which tells each of them
  # to look at the IO itself. A write with none nested in it never asks `tty?`.
  # With the write side closed, a `close_write` has flushed them and the rest go
  # nowhere. All are taken before they are written, as a thread may flush too.
  # `@wsynced` is `sync` as it reached the write side, for `__write_gathered__`:
  # a `sync=` after a `close_write` reaches only the read side, and no later
  # flush comes here.
  def __flush_gathered__
    return if @wclosed || @wgathered.nil? || @wgathered.empty?
    return @wgathered = nil if tty?
    @wsynced = @sync
    bytes = "".b
    @wgathered.each do |text|
      bytes << text
      text.clear
    end
    __write_all__(bytes) unless bytes.empty?
  end

  # The converter gone unfinished, with the quote.
  def __clear_writeconv__
    @writeconv = nil
    @wpre = nil
    __xml_quote__(nil)
  end

  # Where the attribute's quote is, and what the converter is, which the
  # finalizer is told.
  def __xml_quote__(state)
    @xml_quote = state
    @finalizer&.quote = __closing_quote__(state)
    @finalizer&.writeconv = @writeconv
  end

  def __closing_quote__(open)
    return nil if open.nil?
    (open ? "\"" : "\"\"").b
  end

  # The bytes closing writes: the quote goes through the converter, if there
  # is one, for it to write its mark or switch to ASCII for it, and then it
  # finishes.
  def self.__closing__(quote, converter)
    return quote if converter.nil?
    source = converter.source_encoding
    closing = converter.convert((quote || "").dup.force_encoding(source)).b
    closing << converter.finish.b
  end

  # The IOs with a converter to finish, `xml: :attr`'s quote or one a write
  # goes on through, held weakly. CRuby finishes an IO's converter when it
  # finalizes the IO, so the program's end closes the quote of each still open
  # (#456); one collected before then has had its `Finalizer` do it (#460).
  def self.__unfinished__
    @__unfinished__ ||= ObjectSpace::WeakMap.new
  end

  # What finalizing an IO the collector freed does, CRuby's
  # `rb_io_fptr_finalize`: it finishes the write converter, which closes an
  # `xml: :attr` quote and goes back to ASCII, and closes the descriptor,
  # unless it is 0, 1 or 2 or
  # the IO is not to `autoclose` it. It is told what that needs as the IO
  # changes, and never holds the IO, which would keep it alive.
  #
  # At the program's end it does nothing: the IOs still alive then are
  # finished by `IO.__finish_all__`, after every other finalizer, and their
  # descriptors close with the process.
  class Finalizer
    attr_writer :fd, :wfd, :autoclose, :quote, :writeconv

    def disarm
      @fd = @wfd = @quote = @writeconv = nil
    end

    def call(_id)
      return if IO.__ending__
      fd, wfd, quote, writeconv = @fd, @wfd, @quote, @writeconv
      disarm
      unless (quote.nil? && writeconv.nil?) || (wfd || fd).nil?
        # An error finishing is dropped, and the descriptor still closes.
        quote = IO.__closing__(quote, writeconv) rescue "".b
        until quote.empty?
          written = __fd_write__(wfd || fd, quote)
          break unless Array === written && written[0] > 0
          quote = quote.byteslice(written[0], quote.bytesize - written[0])
        end
      end
      __fd_close__(wfd) unless wfd.nil?
      __fd_close__(fd) if @autoclose && !fd.nil? && fd > 2
      nil
    rescue Exception
      nil
    end
  end
  private_constant :Finalizer

  # Whether `callable` is an IO's own finalizer, which
  # `ObjectSpace.undefine_finalizer` leaves: CRuby's is no finalizer at all.
  def self.__finalizer__?(callable) = Finalizer === callable

  def self.__ending__ = @__ending__
  def self.__end__ = (@__ending__ = true; nil)

  # After the finalizers, as CRuby's IOs are finished after every other
  # object's. A failure is dropped, as a finalizer's is.
  def self.__finish_all__
    __unfinished__.keys.each do |io|
      io.__finish__
    rescue Exception
    end
    nil
  end

  def __finish__
    __finish_writeconv__ unless @closed || @wclosed
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
  # ponytail: 128KB only for a stream with options, where `__next_chars__` gives
  # the converter all that is buffered; every other fill is 64KB, so bytes
  # buffered before `set_encoding` makes that a stream with options stay short
  # of CRuby's 128KB. The upgrade is a line read that does not cost with the
  # buffer, and 128KB for any read converter.
  def __fill__(want = __stream_in__? && !__plain_readconv__? ? READ_CONVERTER_BYTES : 65536)
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
  # What writes in progress have gathered goes first, as it is in CRuby's write
  # buffer ahead of these bytes.
  def __write_all__(text)
    __flush_gathered__ if @wgathered && !@wgathered.empty?
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

  def __held_bytes__ = @rbuf.bytesize

  def __take__(count)
    count = @rbuf.bytesize if count > @rbuf.bytesize
    taken = @rbuf.byteslice(0, count)
    @rbuf = @rbuf.byteslice(count, @rbuf.bytesize - count)
    taken
  end

  # Whether the first `cut` bytes of `buffer` end part way through a UTF-8
  # character: a lead byte with fewer bytes after it than it announces.
  def __partial_character__(cut, buffer)
    back = 1
    while back <= 4 && back <= cut
      byte = buffer.getbyte(cut - back)
      if byte & 0xC0 != 0x80
        return false if byte < 0xC0
        needed = byte >= 0xF0 ? 4 : (byte >= 0xE0 ? 3 : 2)
        return back < needed
      end
      back += 1
    end
    false
  end

  # Universal newline on the way in, when it was asked for. A String that is
  # not ASCII-compatible, read into UTF-16 or UTF-32, is gone through a
  # character at a time, as its CR and LF are more than a byte (#462).
  def __newlines_in__(text)
    return text unless __universal_newline__? && !__stream_in__?
    return String.__universal_newline__(text) if text.encoding.ascii_compatible?
    cr = "\r".encode(text.encoding)
    lf = "\n".encode(text.encoding)
    out = String.new(encoding: text.encoding)
    after_cr = false
    text.each_char do |char|
      if char == cr
        out << lf
      elsif !(after_cr && char == lf)
        out << char
      end
      after_cr = char == cr
    end
    out
  end

  # --- reading -------------------------------------------------------------

  def read(length = nil, buffer = nil)
    __readable__
    __readconv__ if length.nil?
    __byte_readable__ unless length.nil?
    unless buffer.nil?
      buffer = buffer.to_str unless String === buffer
      raise FrozenError, "can't modify frozen String: #{buffer.inspect}" if buffer.frozen?
    end
    if length.nil?
      nil while __fill__
      held = __chars_read__? ? __take_chars__ : "".b
      __unpend__
      if __finished_in__?
        # A converter that has finished reads nothing, and goes.
        __finish_in__
        out = held.force_encoding(__read_encoding__)
        return out if buffer.nil?
        return buffer.replace(out)
      end
      begin
        # A replacement past a lone CR comes before its `"\n"`, as the
        # transcoder that comes first makes it (#626).
        bytes = __take__(@rbuf.bytesize)
        out = if __replaces_past_cr__? && __universal_newline__? && (__replaces_invalid__? || @ecopts[:undef] == :replace)
          __decode__(bytes, bytes, true)
        else
          __newlines_in__(__decode__(bytes))
        end
      rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError
        __take_chars__
        raise
      end
      finish = __finish_in__
      __finish_in__ unless finish.nil?
      unless held.empty? && finish.nil?
        held << out.b
        held << finish.b unless finish.nil?
        out = held.force_encoding(out.encoding)
      end
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
    __byte_readable__
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
    __byte_readable__
    raise IOError, "sysread for buffered IO" unless @rbuf.empty?
    # Measured: a zero length hands the buffer back as it was.
    return buffer || "" if Integer.__index__(length) == 0
    readpartial(length, buffer)
  end

  def pread(length, offset, buffer = nil)
    __readable__
    __byte_readable__
    length = Integer.__index__(length)
    offset = Integer.__index__(offset)
    raise ArgumentError, "negative string size (or size too big)" if length < 0
    # Measured: a zero length hands the buffer back as it was.
    return buffer || "" if length == 0
    chunk = __pread_bytes__(length, offset)
    raise EOFError, "end of file reached" if chunk.empty?
    return chunk if buffer.nil?
    # The caller's buffer keeps its encoding.
    encoding = buffer.encoding
    buffer.replace(chunk).force_encoding(encoding)
  end

  # `IO.copy_stream` names the call where `pread` names the path.
  def __pread_bytes__(length, offset, path = @path)
    IO.__check__(__fd_pread__(@fileno, length, offset), path)
  end

  # Where the descriptor is, whatever is buffered.
  def __fd_pos__
    IO.__check__(__fd_seek__(@fileno, 0, IO::SEEK_CUR), @path)[0]
  end

  def __fd_pos__=(offset)
    IO.__check__(__fd_seek__(@fileno, offset, IO::SEEK_SET), @path)
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
    __check_open__
    __check_separator__(separator)
    [separator, limit]
  end

  # CRuby's `check_getline_args`: a separator is looked for in what the
  # read converter makes, so one in another encoding raises, unless it is
  # ASCII on an ASCII-compatible read, or empty (#482). Measured: before
  # the IO is checked for reading and before a limit of 0. The default
  # separator, CRuby's `rb_default_rs`, is let through by identity and
  # stands for a newline in the read encoding; the `$/` setter puts it
  # back for any `"\n"` (#484).
  def __check_separator__(separator)
    return if separator.nil?
    enc = __read_encoding__
    return if separator.encoding == enc
    return if separator.ascii_only? && (separator.empty? || enc.ascii_compatible?)
    return if separator.equal?(IO.__default_rs__)
    raise ArgumentError, "encoding mismatch: #{enc} IO with #{separator.encoding} RS"
  end

  def __gets__(separator, limit, chomp)
    __readable__
    return "" if limit == 0
    __readconv__
    if separator.nil? && limit.nil?
      line = read
      return nil if line.empty?
      @lineno += 1
      $. = @lineno
      return line
    end
    # An empty separator is paragraph mode: a blank line ends the record.
    paragraph = !separator.nil? && separator.empty?
    # With universal newline a CR is a newline too, alone or before a LF.
    unit = __universal_newline__? ? __newline_unit__ : nil
    # With a read converter CRuby reads a record from what the converter
    # makes: the blank lines before it are skipped there, characters held
    # among them, and the limit counts what it made, so a limit of 2 is
    # one character in UTF-16 (#477). So does a limit on any line where the
    # converter makes other bytes than it reads (#479).
    stream = __stream_in__?
    kept = @rkept || __newline_on_bytes__?
    # So does one with a converter that has finished, which it finds (#551).
    # A read without options has no decorator, and looks no further.
    options = !@ecopts.empty?
    converted = paragraph ? __read_converter__? : stream || __chars_read__? || options && __xml_in__ || @rfinished && __read_converter__? || !limit.nil? && __converts_bytes__?
    # Blank lines before a record are not part of it.
    __swallow__(unit) if paragraph
    wanted = nil
    unless separator.nil? || stream
      wanted = paragraph ? "\n\n" : separator
      begin
        wanted = wanted.encode(__encoding__) unless wanted.encoding == __encoding__ || wanted.ascii_only? && __encoding__.ascii_compatible?
        wanted = wanted.b
      rescue EncodingError
        raise if @enc2.nil?
        wanted = nil
      end
      # CRuby's `at_char_boundary`: with an internal encoding the separator
      # is looked for in what the converter makes, and only where a
      # character starts. Where its bytes as read could be found inside a
      # character, or it has none, the line is read from what the
      # converter makes, a line of bytes at a time (#552).
      if !@enc2.nil? && (wanted.nil? || !__starts_char__?(wanted))
        converted = true
        wanted = __newline_read__
      end
    end
    if converted
      line = __converted_line__(separator, wanted, limit, unit)
      return nil if line.nil?
    elsif unit
      bytes, cr, taken = __universal_line__(wanted, limit, unit, true)
      quote = @rquote
      line = bytes.nil? ? nil : __decode__(bytes, taken)
      # A line with a byte to replace goes back, CRs and all, to be read as
      # the converter makes it, its universal newline in it (#610), as does
      # one that opens an attribute on a replacement (#636).
      if line && options && (__replaces_invalid__? && !bytes.valid_encoding? || __opens_on_replacement__?(bytes, quote))
        @rbuf = taken.b + @rbuf
        line = __converted_line__(separator, wanted, limit, unit)
      elsif cr
        line = __before_newline__(line, __read_ahead__(cr, unit), cr, unit)
      end
    else
      bytes = __line_bytes__(wanted, limit, true)
      line = bytes.nil? ? nil : __newlines_in__(__decode__(bytes))
      # A line with a byte to replace goes back, to be read as the converter
      # makes it, which `__moved_past__?` sends on past it (#609).
      if line && options && __replaces_invalid__? && !bytes.valid_encoding?
        @rbuf = bytes.b + @rbuf
        line = __converted_line__(separator, wanted, limit, unit)
      end
    end
    return nil if line.nil?
    # CRuby's `nolimit`: a line the limit cut short is not counted (#480).
    counted = !__cut_by_limit__?(line, separator, limit) || kept && __ends_cut_short__?(line, line.encoding)
    # The blank lines between records belong to neither.
    __swallow__(unit) if paragraph
    line = __chomp_line__(line, separator) if chomp && !separator.nil?
    if counted
      @lineno += 1
      $. = @lineno
    end
    line
  end

  # Whether the bytes of a separator as read are found only where a
  # character starts. In UTF-8 and single-byte encodings they are; in the
  # others a byte below "0" is no part of a character of more, so it starts
  # one, while a letter or "\\" can end one in Shift_JIS, Big5 or GBK.
  def __starts_char__?(wanted)
    enc = __encoding__
    return true unless enc.ascii_compatible?
    enc == Encoding::UTF_8 || enc.__single_byte__ || wanted.getbyte(0) < 0x30
  end

  # A newline in the bytes as read, to read a line of them at a time.
  def __newline_read__
    __encoding__.ascii_compatible? ? "\n".b : "\n".encode(__encoding__).b
  end

  # Whether the read converter makes other bytes than it reads: an internal
  # encoding, or an external one that is not ASCII-compatible.
  def __converts_bytes__?
    __read_converter__? && (!@enc2.nil? || !__encoding__.ascii_compatible?)
  end

  # Whether the limit, rather than the separator or the end, ended a line
  # as read: it is as long as the limit, counted in what the read converter
  # made, and does not end with the separator.
  def __cut_by_limit__?(line, separator, limit)
    return false if limit.nil? || limit <= 0 || line.bytesize < limit
    return true if separator.nil?
    !__ends_with_separator__?(line, __separator_in__(separator, line.encoding))
  end

  # Whether a line ends with the bytes of its separator where a character
  # of it starts, CRuby's `at_char_boundary` (#552).
  def __ends_with_separator__?(line, tail)
    bytes = line.b
    size = bytes.bytesize - tail.bytesize
    bytes.end_with?(tail) && __char_edge__(bytes, 0, size, line.encoding) == size
  end

  # The bytes of a separator in what the read converter makes. A dummy
  # encoding is read a byte at a time, so its newline is the byte, even in a
  # BOM'd UTF-16 or UTF-32. Paragraph mode's empty separator is CRuby's
  # "\n\n" converted to the read encoding, so in a BOM'd one it is the BOM
  # and two newlines in its units, and a record ends only where those bytes
  # are (#542). Measured.
  def __separator_in__(separator, enc)
    paragraph = separator.empty?
    separator = "\n\n" if paragraph
    return separator.b if separator.encoding == enc || separator.ascii_only? && (enc.ascii_compatible? || enc.dummy? && !paragraph)
    separator.encode(enc).b
  end

  # The next line as the read converter makes it, or nil at the end, where
  # it holds characters or decorates, every record in paragraph mode
  # through one (#477), and a line with a limit where it makes other bytes
  # than it reads (#479): CRuby looks for the separator, and
  # counts the limit, in what its converter made, so `xml:`'s entities and
  # quote count and a separator can be in them, and the characters made
  # past the line are held for the next read (#466, #468, #470). `wanted`
  # is the separator in the bytes read, for reading a line of them at a
  # time; at the end, the converter's last characters end the line.
  # ponytail: a line of bytes is read to its own separator, which can be
  # past where the separator is found in what it makes, so `pos` can be
  # ahead of CRuby's there; upgrade by converting a character at a time.
  def __converted_line__(separator, wanted, limit, unit)
    enc = __read_encoding__
    sep = separator.nil? ? nil : __separator_in__(separator, enc)
    out = "".b
    from = 0
    # Where a character of `out` is known to start, for CRuby's
    # `at_char_boundary`: a separator counts only at a character's start (#552).
    edge = 0
    cut = nil
    stream = __stream_in__?
    runs = stream && __line_runs__?(sep, enc)
    goes_on = !limit.nil? && limit > 0
    while cut.nil?
      # `out` is bytes: `String#index` would tabulate every character of it
      # on each search, which grows with the record (#585).
      at = sep.nil? ? nil : out.__byte_index__(sep, from)
      until at.nil? || (edge = __char_edge__(out, edge, at, enc)) == at
        at = out.__byte_index__(sep, at + 1)
      end
      if at && (limit.nil? || limit <= 0 || at + sep.bytesize <= limit)
        cut = at + sep.bytesize
      elsif !limit.nil? && limit > 0 && out.bytesize >= limit && (cut = __limit_cut__(out, limit, enc))
        next
      elsif __chars_read__?
        from = sep.nil? ? 0 : [out.bytesize - sep.bytesize + 1, 0].max
        out << __take_chars__
      else
        from = sep.nil? ? 0 : [out.bytesize - sep.bytesize + 1, 0].max
        room = limit.nil? || limit <= 0 ? limit : [limit - out.bytesize, 1].max
        if stream
          piece = runs ? __chars_to_line_end__(sep, room) : __next_chars__(goes_on)
        elsif @rfinished
          piece = nil
        elsif unit
          bytes, cr, taken = __universal_line__(wanted, room, unit)
          quote = @rquote
          piece = bytes.nil? ? nil : __decode__(bytes, taken)
          if piece && !@ecopts.empty? && (__moved_past__?(piece, bytes, sep, wanted, enc) || __opens_on_replacement__?(bytes, quote))
            # The bytes as read go back, CRs and all, for the converter's
            # universal newline to make them as CRuby's does (#610, #636).
            __stream_from__(taken, quote)
            stream = true
            piece = __next_chars__(goes_on)
          else
            made = __within_limit__(out, piece, bytes, taken, limit, enc) if piece && room && room > 0 && piece.bytesize > limit - out.bytesize
            piece, cr = made, nil if made
            piece = __before_newline__(piece, __read_ahead__(cr, unit), cr, unit) if cr
          end
        else
          bytes = __line_bytes__(wanted, room)
          quote = @rquote
          piece = bytes.nil? ? nil : __newlines_in__(__decode__(__rest_of_char__(bytes, wanted)))
          if piece && !@ecopts.empty? && __moved_past__?(piece, bytes, sep, wanted, enc)
            __stream_from__(bytes, quote)
            stream = true
            piece = __next_chars__(goes_on)
          elsif piece && room && room > 0 && piece.bytesize > limit - out.bytesize
            piece = __within_limit__(out, piece, bytes, bytes, limit, enc) || piece
          end
        end
        if piece.nil?
          # At the end, until the converter has nothing more.
          finish = __finish_in__
          if finish.nil?
            cut = out.bytesize
          else
            out << finish.b
          end
        else
          out << piece.b
        end
      end
    end
    return nil if cut == 0
    __hold_chars__(out.byteslice(cut, out.bytesize - cut)) if cut < out.bytesize
    __leave_stream__ if stream
    out.byteslice(0, cut).force_encoding(enc)
  end

  # Reads go a line of bytes at a time again once the converter they went
  # through after a replacement without `xml:` (#609) holds nothing: a new
  # one, as `String#encode` makes, goes as it would, and the next line with
  # a byte to replace comes back to it. Otherwise every line after one bad
  # byte would go a character at a time, ten times slower. With `xml:` the
  # converter keeps an attribute's quote, so it stays (#566).
  def __leave_stream__
    return unless @rstream && __xml_in__.nil? && @readconv&.__idle__?
    @rstream = false
    @readconv = nil
  end

  # Whether the decorator moved what came before the separator in a line of
  # bytes past it in what they made: `xml:`'s buffer puts a replacement after
  # what it has still to read, and the bytes an error left to read again after
  # that (#543), and one from past the separator can come before it. CRuby's
  # converter is given all that is buffered and does not stop for a
  # replacement, so the line ends where the separator is found in what it
  # makes of them, and `pos` is past them all (#566). So any line with a byte
  # to replace goes that way. Without `xml:` nothing moves, but the converter
  # still goes on past the separator after a replacement, so `pos` is past
  # what it went on to (#609). Measured.
  def __moved_past__?(piece, bytes, sep, wanted, enc)
    return false if sep.nil? || wanted.nil?
    xml = __xml_in__
    # Without an internal encoding only a decorator converts.
    return !xml.nil? || !@enc2.nil? unless bytes.valid_encoding?
    return false if xml.nil?
    # Valid bytes are replaced only where they have no character there.
    return false if @ecopts[:undef] != :replace || __ends_in__?(piece, sep) || !__ends_in__?(bytes, wanted)
    made = piece.b
    at = made.index(sep)
    at = made.index(sep, at + 1) until at.nil? || __char_edge__(made, 0, at, enc) == at
    !at.nil?
  end

  # Whether a line of bytes opens an attribute's quote on a character the
  # internal encoding has no spelling for, under universal newline: CRuby's
  # converter has its newline decorator before the `xml:` one, so the
  # replacement comes out before the quote opens, where `String#encode`
  # puts the quote first (#636). Measured.
  def __opens_on_replacement__?(bytes, quote)
    return false if quote || @enc2.nil? || __xml_in__ != :attr || !__universal_newline__?
    char = bytes.byteslice(0, 4).force_encoding(__encoding__)[0]
    return false if char.nil? || !char.valid_encoding?
    char.encode(__read_encoding__)
    false
  rescue Encoding::UndefinedConversionError
    true
  end

  # Whether the read converter replaces bytes that make no character: an
  # internal encoding and `invalid: :replace`. Without one nothing converts.
  def __replaces_invalid__?
    !@enc2.nil? && @ecopts[:invalid] == :replace
  end

  # Whether `text` ends in the bytes of `tail`, a byte at a time: a copy of
  # either costs more than the line's read (#588).
  def __ends_in__?(text, tail)
    size = text.bytesize - tail.bytesize
    return false if size < 0
    at = 0
    while at < tail.bytesize
      return false unless text.getbyte(size + at) == tail.getbyte(at)
      at += 1
    end
    true
  end

  # The bytes of a line go back, and the IO reads through its own converter
  # from there, as a stream does, until it drops it: a line of bytes at a time
  # finishes a conversion that CRuby's goes on with. One that opened an
  # attribute's quote before has it open.
  def __stream_from__(bytes, quote)
    @rbuf = bytes.b + @rbuf
    @rquote = quote
    @rstream = true
    @readconv = __make_readconv__
    @readconv.primitive_convert("a".encode(__encoding__), "".b, nil, nil, Encoding::Converter::PARTIAL_INPUT) if @rquote
  end

  # The start of the character of `out` that `at` is in, or `at` itself
  # where one starts there, walking from `edge`, where one does. In UTF-16
  # or UTF-32 that is a unit's start; in UTF-8 any byte that does not go
  # on with one starts one.
  def __char_edge__(out, edge, at, enc)
    return at if enc.dummy? || enc.__single_byte__
    if !enc.ascii_compatible?
      step = enc.name.start_with?("UTF-32") ? 4 : 2
      return at - at % step
    end
    if enc == Encoding::UTF_8
      at -= 1 while at > edge && out.getbyte(at) & 0xC0 == 0x80
      return at
    end
    out.byteslice(edge, at - edge + 4).force_encoding(enc).each_char do |char|
      break if edge + char.bytesize > at
      edge += char.bytesize
    end
    edge
  end

  # Where a limit ends a line of what the read converter made: after the
  # character it lands in.
  def __limit_cut__(out, limit, enc)
    return __kept_limit_cut__(out, limit, enc) if @rkept || __newline_on_bytes__?
    cut = 0
    out.dup.force_encoding(enc).each_char { |char| break if cut >= limit; cut += char.bytesize }
    cut
  end

  # CRuby's `more_char` converts a character at a time, so a limit leaves
  # what the line did not take as read, not converted and held in `cbuf`,
  # where `reopen` would keep it and a read of bytes would refuse (#556).
  # `piece` was made of `bytes`, the line as read, or of `taken` where
  # universal newline made `bytes` of it, and goes past the limit. The
  # characters read up to the one whose making reaches the limit are taken,
  # and the rest go back to be read; what that last one made past the limit
  # is held, as in `cbuf`. The piece is what they made; nil where it is held
  # as a whole. Where a character read makes other than one, an `xml:`
  # entity or a replacement of more, what each made is found by converting
  # it alone (#602). Measured.
  def __within_limit__(out, piece, bytes, taken, limit, enc)
    need = limit - out.bytesize
    made = piece.dup.force_encoding(enc)
    read = bytes.dup.force_encoding(__encoding__)
    size = made.length
    length = read.length
    if length == size
      # The characters up to the one the limit lands in. Each byte of one it
      # cuts short counts as one.
      count = made.byteslice(0, need).length
      count -= 1 while count > 1 && made[0, count - 1].bytesize >= need
      return nil if count >= size
      upto = made[0, count].bytesize
    else
      count, upto, sizes = __made_within__(read, made, need)
      return nil if count.nil?
    end
    # A replacement for an invalid byte goes in before the converter's last
    # step where that writes what is not ASCII-compatible, so the step goes
    # on to make the next character with it. Measured.
    unless __read_encoding__.ascii_compatible?
      until read[count - 1].valid_encoding?
        return nil if count >= length
        upto += sizes ? sizes[count] : made[count].bytesize
        count += 1
      end
    end
    read = read[0, count]
    back = taken.bytesize == bytes.bytesize ? read.bytesize : __taken_of__(read, taken)
    @rbuf = taken.byteslice(back, taken.bytesize - back).b << @rbuf
    made.byteslice(0, upto)
  end

  # How many characters of `read` make what reaches `need` bytes of `made`,
  # how many bytes of it they make, and what each makes: each is converted
  # alone, as `__decode__` converts them together, and an attribute's quote
  # that opens comes with the first. Nil where all of them are needed, or
  # where what they make alone does not add up to what they made.
  def __made_within__(read, made, need)
    xml = __xml_in__
    lead = made.bytesize
    sizes = []
    read.each_char do |char|
      one = __made_alone__(char, xml)
      return nil if one.nil?
      sizes << one.bytesize
      lead -= one.bytesize
    end
    return nil unless lead == 0 || xml == :attr && lead == "\"".encode(made.encoding).bytesize
    count = 0
    upto = lead
    loop do
      upto += sizes[count]
      count += 1
      break if upto >= need
    end
    return nil if count >= sizes.size
    [count, upto, sizes]
  end

  # What a character read makes on its own through the read converter, as
  # `__decode__` makes it, without an attribute's quotes; nil where it raises.
  def __made_alone__(char, xml)
    if @enc2.nil?
      return char if xml.nil?
      one = String.__escape_xml__(char, xml)
    else
      one = char.encode(__read_encoding__, **__read_options__(@ecopts))
    end
    xml == :attr ? one[1...-1] : one
  rescue EncodingError
    nil
  end

  # How many bytes of `taken` the characters of `read` were made of: with
  # universal newline each LF that stood for a CRLF is a unit more.
  def __taken_of__(read, taken)
    lf = "\n".encode(__encoding__)
    cr = "\r".encode(__encoding__).b
    unit = lf.bytesize
    at = 0
    read.each_char do |char|
      at += unit if char == lf && taken.byteslice(at, unit) == cr && taken.byteslice(at + unit, unit) == lf.b
      at += char.bytesize
    end
    at
  end

  # What a converter `reopen` left or a `NewlineBytes` makes need not end where
  # a character of what it is called does, so CRuby's `extra_limit` goes a byte past the limit
  # while the bytes up to it end in NEEDMORE. Nil where it needs more made; at
  # the end of the bytes the line is what there is, and the limit did not end
  # it. Measured.
  def __kept_limit_cut__(out, limit, enc)
    cut = limit
    while __ends_cut_short__?(out.byteslice(0, cut), enc)
      return nil if cut == out.bytesize
      cut += 1
    end
    cut
  end

  # Whether `line` ends in a character of `enc` cut short. One reads as a byte
  # at a time, so each start is asked.
  def __ends_cut_short__?(line, enc)
    at = 0
    line.dup.force_encoding(enc).each_char do |char|
      return true if __held_cut_short__?(line.byteslice(at, 4), enc)
      at += char.bytesize
    end
    false
  end

  # The rest of the character a line of bytes read stops inside, where the
  # limit counted what the converter makes rather than the bytes read: in
  # UTF-16 or UTF-32 a whole unit, and the low surrogate after a high one
  # (#477); in another multibyte encoding, the bytes it is cut short of (#553).
  def __rest_of_char__(bytes, wanted)
    unit = __newline_unit__
    return __rest_of_multibyte__(bytes, wanted) if unit.nil? || unit == 1
    want = -bytes.bytesize % unit
    if want == 0 && unit == 2 && bytes.bytesize >= 2
      high = __encoding__ == Encoding::UTF_16BE ? bytes.getbyte(-2) : bytes.getbyte(-1)
      want = 2 if high & 0xFC == 0xD8
    end
    while want > 0 && (!@rbuf.empty? || __fill__)
      more = __take__(want)
      bytes << more
      want -= more.bytesize
    end
    bytes
  end

  # `bytes` with the rest of a character of an ASCII-compatible multibyte
  # encoding other than UTF-8 its end is cut short of. `__line_bytes__`
  # already does it for UTF-8, and a line that ends in its separator ends
  # where a character does.
  def __rest_of_multibyte__(bytes, wanted)
    enc = __encoding__
    return bytes if enc == Encoding::UTF_8 || enc.dummy? || enc.__single_byte__
    return bytes if wanted && bytes.end_with?(wanted)
    extra = 0
    while extra < 4 && __last_cut_short__?(bytes, enc) && (!@rbuf.empty? || __fill__)
      bytes << __take__(1)
      extra += 1
    end
    bytes
  end

  # Whether the last character of `bytes`, read as `enc`, is cut short. A
  # broken one reads as a character per byte, GB18030's "\x81\x30" as "\x81"
  # and "0", so the tail from each of the last three starts is asked.
  def __last_cut_short__?(bytes, enc)
    tail = []
    bytes.dup.force_encoding(enc).each_char do |char|
      tail << char.b
      tail.shift if tail.size > 3
    end
    (0...tail.size).any? { |at| __held_cut_short__?(tail.drop(at).join.b, enc) }
  end

  # The bytes of the next line as read, or nil at the end: up to the
  # separator, or `limit` bytes, or the end.
  def __line_bytes__(wanted, limit, whole = false)
    from = 0
    cut = nil
    # In UTF-16 or UTF-32 a separator counts only at a character's start.
    step = @enc2.nil? ? 1 : (__newline_unit__ || 1)
    # With no read converter too, CRuby's `at_char_boundary` takes it only
    # where a character of the external encoding starts, so a "\\" that
    # ends one in Shift_JIS, Big5 or GBK is no separator (#597). `edge` is
    # where one is known to start, walked from the buffer's start.
    chars = __encoding__ unless wanted.nil? || __starts_char__?(wanted)
    edge = 0
    while cut.nil?
      # The buffer is bytes: `String#index` would tabulate every character
      # of it on each line, so a file a line at a time was quadratic (#585).
      at = wanted.nil? ? nil : @rbuf.__byte_index__(wanted, from)
      at = @rbuf.__byte_index__(wanted, at + 1) while at && at % step != 0
      until chars.nil? || at.nil? || (edge = __char_edge__(@rbuf, edge, at, chars)) == at
        at = @rbuf.__byte_index__(wanted, at + 1)
      end
      if at
        cut = at + wanted.bytesize
      elsif !limit.nil? && limit > 0 && @rbuf.bytesize >= limit
        cut = limit
      else
        from = @rbuf.bytesize - (wanted.nil? ? 0 : wanted.bytesize) + 1
        from = 0 if from < 0
        unless __fill__
          cut = @rbuf.bytesize
          __read_ended__ if whole
        end
      end
    end
    if !limit.nil? && limit > 0 && limit <= cut
      cut = limit
      # A limit that lands inside a character takes the rest of it: Ruby
      # does not hand back half a character. In UTF-8, where the bytes
      # that continue one are recognisable, and UTF-16 or UTF-32, where the
      # character the limit lands in starts on a unit, as `BOM|` that finds
      # no mark reads (#564).
      enc = __encoding__
      if enc == Encoding::UTF_8
        extra = 0
        while extra < 16 && __partial_character__(cut, @rbuf) && (cut < @rbuf.bytesize || __fill__)
          cut += 1
          extra += 1
        end
      elsif !enc.ascii_compatible? && (unit = __newline_unit__)
        loop do
          at = cut - 1 - (cut - 1) % unit
          at -= unit if unit == 2 && at >= unit && (@rbuf.getbyte(enc == Encoding::UTF_16LE ? at + 1 : at) || 0) & 0xFC == 0xDC
          break unless __held_cut_short__?(@rbuf.byteslice(at, cut - at), enc) && (cut < @rbuf.bytesize || __fill__)
          cut += 1
        end
      elsif !enc.dummy? && !enc.__single_byte__
        # In Shift_JIS, EUC-JP, Big5, GBK and the like to the end of the
        # character, as the universal newline path does (#553, #628).
        extra = 0
        while extra < 4 && __last_cut_short__?(@rbuf.byteslice(0, cut), enc) && (cut < @rbuf.bytesize || __fill__)
          cut += 1
          extra += 1
        end
      end
    end
    cut == 0 ? nil : __take__(cut)
  end

  # The bytes of the next line with universal newline, converted, or nil at
  # the end. CRuby converts what it reads before it looks for the separator
  # or counts the limit, so a lone CR ends a line too (#464). The buffer
  # keeps the bytes as they were read, as `pos`, `read(n)` and `getbyte`
  # after a line see them in CRuby: the line is converted only as far as it
  # goes, and each LF that stood for a CRLF is a byte more it takes. It answers
  # those bytes as well, for what goes back if converting the line raises.
  def __universal_line__(wanted, limit, unit, whole = false)
    __unpend__
    enc, cr, lf, newline = __newline_bytes__(unit)
    line = { view: "".b, crlfs: [], lone: [], scanned: 0, ended: false, end_cr: nil, cr: cr, lf: lf, unit: unit, newline: newline }
    view = line[:view]
    from = 0
    cut = nil
    # In Shift_JIS, Big5 or GBK only where a character starts, as in
    # `__line_bytes__` (#629).
    chars = enc unless wanted.nil? || __starts_char__?(wanted)
    edge = 0
    while cut.nil?
      at = wanted.nil? ? nil : view.index(wanted, from)
      # In UTF-16 or UTF-32 only at a character's start.
      at = view.index(wanted, at + 1) while at && at % unit != 0
      until chars.nil? || at.nil? || (edge = __char_edge__(view, edge, at, chars)) == at
        at = view.index(wanted, at + 1)
      end
      if at
        cut = at + wanted.bytesize
      elsif !limit.nil? && limit > 0 && view.bytesize >= limit
        cut = limit
      else
        from = view.bytesize - (wanted.nil? ? 0 : wanted.bytesize) + 1
        from = 0 if from < 0
        unless __convert_more__(line)
          cut = view.bytesize
          line[:end_cr] = nil
          __read_ended__ if whole
        end
      end
    end
    if !limit.nil? && limit > 0 && limit <= cut
      cut = limit
      if enc == Encoding::UTF_8
        extra = 0
        while extra < 16 && __partial_character__(cut, view) && (cut < view.bytesize || __convert_more__(line))
          cut += 1
          extra += 1
        end
      elsif unit > 1
        # In UTF-16 or UTF-32 to the end of the character, and of a
        # surrogate pair (#477).
        cut += -cut % unit
        nil while view.bytesize < cut && __convert_more__(line)
        if unit == 2 && cut >= 2 && cut <= view.bytesize
          high = enc == Encoding::UTF_16BE ? view.getbyte(cut - 2) : view.getbyte(cut - 1)
          if high & 0xFC == 0xD8
            nil while view.bytesize < cut + 2 && __convert_more__(line)
            cut += 2
          end
        end
        cut = view.bytesize if cut > view.bytesize
      elsif !enc.dummy? && !enc.__single_byte__
        # In another multibyte encoding to the end of the character (#553).
        extra = 0
        while extra < 4 && __last_cut_short__?(view.byteslice(0, cut), enc) && (cut < view.bytesize || __convert_more__(line))
          cut += 1
          extra += 1
        end
      end
    end
    return nil if cut == 0
    taken = __take__(cut + line[:crlfs].count { |at| at < cut } * unit)
    # A line that ends in a lone CR had the character after it read: the
    # CR, for the caller to hold that character once the line is made. So
    # did one that ends in a CR the bytes end in, and found the end (#551).
    [view.byteslice(0, cut), line[:lone].include?(cut) || line[:end_cr] == cut ? cr : nil, taken]
  end

  # Converts the buffered bytes of a line up to the next newline, reading
  # more when there are none; false at the end. A CR the buffer ends in
  # waits for what comes after it, which says whether a LF goes with it, as
  # CRuby's converter does, on a pipe as well.
  def __convert_more__(line)
    view = line[:view]
    unit = line[:unit]
    loop do
      scanned = line[:scanned]
      at = @rbuf.index(line[:newline], scanned)
      at = @rbuf.index(line[:newline], at + 1) while at && at % unit != 0
      upto = nil
      if at.nil?
        # Whole characters only, until the end.
        upto = line[:ended] ? @rbuf.bytesize : @rbuf.bytesize / unit * unit
      elsif @rbuf.byteslice(at, unit) == line[:lf]
        upto = at + unit
      elsif line[:ended] || @rbuf.bytesize >= at + 2 * unit
        crlf = @rbuf.byteslice(at + unit, unit) == line[:lf]
        view << @rbuf.byteslice(scanned, at - scanned)
        line[:crlfs] << view.bytesize if crlf
        view << line[:lf]
        line[:lone] << view.bytesize if !crlf && @rbuf.bytesize >= at + 2 * unit
        line[:end_cr] = view.bytesize if line[:ended] && @rbuf.bytesize == at + unit
        line[:scanned] = at + (crlf ? 2 : 1) * unit
        return true
      else
        upto = at
      end
      if upto > scanned
        view << @rbuf.byteslice(scanned, upto - scanned)
        line[:scanned] = upto
        return true
      end
      return false if line[:ended]
      line[:ended] = !__fill__
    end
  end

  # The bytes a CR or LF is in the encoding read, for universal newline on
  # lines; nil for a dummy encoding, whose lines are converted afterwards.
  def __newline_unit__
    enc = __encoding__
    return 1 if enc.ascii_compatible?
    case enc
    when Encoding::UTF_16LE, Encoding::UTF_16BE then 2
    when Encoding::UTF_32LE, Encoding::UTF_32BE then 4
    end
  end

  # The encoding read, a CR and a LF in it, and a Regexp for either, kept
  # until the encoding changes.
  def __newline_bytes__(unit)
    enc = __encoding__
    return @newline_bytes if @newline_bytes && @newline_bytes[0] == enc
    cr, lf = unit == 1 ? ["\r".b, "\n".b] : ["\r".encode(enc).b, "\n".encode(enc).b]
    @newline_bytes = [enc, cr, lf, Regexp.union(cr, lf)]
  end

  # Takes the blank lines before or between records in paragraph mode. With
  # universal newline a CR is one too, and each is a whole character, so
  # the buffer stays at a character's start in UTF-16 or UTF-32. A read
  # converter looks for them in what it makes, so a character at a time
  # there without universal newline too (#475); with none they are bytes.
  def __skip_blank__(unit)
    universal = !unit.nil?
    unit ||= __newline_unit__ if __read_converter__? && !__encoding__.ascii_compatible?
    unless unit
      __take__(1) while (@rbuf.empty? ? __fill__ : true) && @rbuf.getbyte(0) == 10
      return
    end
    __unpend__
    cr, lf = __newline_bytes__(unit)[1, 2]
    loop do
      next if @rbuf.bytesize < unit && __fill__
      head = @rbuf.byteslice(0, unit)
      break unless head == lf || (universal && head == cr)
      __take__(unit)
    end
  end

  # CRuby's `swallow`: the blank lines before or after a record in
  # paragraph mode. The newlines the read converter holds go first; when
  # that leaves nothing held, the blank lines read, and the character after
  # them is held. At the end of the bytes that goes for what the converter
  # makes as it finishes too, a CR it held among them; a line that read to
  # the end has finished it, and CRuby's has no blank lines to swallow.
  # Measured.
  def __swallow__(unit)
    stream = __stream_in__?
    while __chars_read__? || stream
      if __chars_read__?
        lf = __separator_in__("\n", __read_encoding__)
        held = @cbuf.b
        at = 0
        at += lf.bytesize while held.byteslice(at, lf.bytesize) == lf
        @cbuf = held.byteslice(at, held.bytesize - at)
        return if __chars_held__?
      end
      next unless stream
      return if @rfinished
      made = __next_chars__ || (__finish_in__ if @readconv)
      return if made.nil?
      __hold_chars__(made.b)
    end
    return if __finished_in__?
    __skip_blank__(unit)
    __hold_next__ if __read_converter__?
  end

  # CRuby's `swallow` skips the blank lines in what the read converter
  # makes, so it converts the character after them and holds it, and `pos`
  # counts it (#475). Without an internal encoding the converter is the
  # newline decorator alone, which makes a character of each byte; with one,
  # a whole character is read, and an invalid one raises. At the end of the
  # bytes it runs the converter to its end, which finishes it, unless the
  # record read that far and dropped it (#551). Measured.
  # ponytail: the character is sized by `valid_encoding?`, so with an invalid
  # sequence the converter replaces nothing is held and `pos` stays short of
  # CRuby's; upgrade by asking the transcoder how many bytes its first
  # character takes.
  def __hold_next__
    if @rbuf.empty? && !__fill__
      __finished_at_end__ unless @rmade.nil?
      return
    end
    size = 1
    unless @enc2.nil?
      enc = __encoding__
      return if enc.dummy?
      while @rbuf.bytesize < 4
        break unless __fill__
      end
      head = @rbuf.byteslice(0, 4).force_encoding(enc)
      size = (1..head.bytesize).find { |n| head.byteslice(0, n).valid_encoding? }
      if size.nil?
        # None is valid: converting them raises the transcoder's error.
        __take__(head.bytesize)
        __decode__(head)
        @rbuf = head.b + @rbuf
        return
      end
    end
    __hold_chars__(__decode__(__take__(size)).b)
  end

  # `chomp:` takes off the separator and nothing else, as CRuby's
  # `rb_io_getline` does, with the CR before a separator that is a single
  # "\n" byte: a lone CR stays, which `String#chomp("\n")` would take
  # (#467). In paragraph mode that is the blank line that ended the record,
  # so the last record keeps its own newline. Measured.
  def __chomp_line__(line, separator)
    tail = __separator_in__(separator, line.encoding)
    return line unless __ends_with_separator__?(line, tail)
    bytes = line.b
    size = bytes.bytesize - tail.bytesize
    size -= 1 if tail.bytesize == 1 && tail.getbyte(0) == 10 && size > 0 && bytes.getbyte(size - 1) == 13
    line.byteslice(0, size)
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
    __byte_readable__
    __next_byte__
  end

  def __next_byte__
    return nil if @rbuf.empty? && !__fill__
    __take__(1).getbyte(0)
  end

  def readbyte
    byte = getbyte
    raise EOFError, "end of file reached" if byte.nil?
    byte
  end

  # The bytes buffered and to come, past the characters the read converter
  # holds, which CRuby's `rb_io_each_byte` does not look at. Measured. As
  # there, whether the IO reads is asked only after a yield: one that does
  # not hands over what its descriptor gives, or fails as the read does.
  def each_byte
    return enum_for(:each_byte) unless block_given?
    __check_open__
    until @rbuf.empty? && !__fillbuf__
      byte = @rbuf.getbyte(0)
      @rbuf = @rbuf.__byteslice__(1, @rbuf.bytesize - 1)
      yield byte
      __readable__
      __byte_readable__
    end
    self
  end

  # CRuby's `io_fillbuf`, whose failure names the descriptor and the path.
  def __fillbuf__
    chunk = __read_wait__(65536)
    raise SystemCallError.new("fd:#{@fileno} #{@path}", chunk, "io_fillbuf") if Integer === chunk
    return false if chunk.empty?
    @rbuf << chunk
    true
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
    __readconv__
    return __kept_char__ if @rkept && __read_converter__? || __newline_on_bytes__?
    if __chars_read__?
      held = @cbuf.dup.force_encoding(__read_encoding__)
      return __take_char__ unless @enc2.nil? && !held.valid_encoding? && held[0].bytesize == held.bytesize
      # The first byte of a longer character, read past a lone CR, is all
      # that is held: it is read again with the bytes after it (#472).
      @rbuf = __take_chars__ << @rbuf
    end
    pended = !@rpend.nil?
    __unpend__
    if __stream_in__?
      made = __next_chars__ || __finish_in__
      return nil if made.nil?
      __hold_chars__(made.b)
      __leave_stream__
      return __take_char__
    end
    return __finish_in__ if @rfinished && __read_converter__?
    if @rbuf.empty? && !__fill__
      finish = __finish_in__
      return nil if finish.nil?
      __hold_chars__(finish.b)
      return __take_char__
    end
    # The lead byte says how many more there are: reading further ahead
    # would wait on a pipe for bytes nobody has written.
    nil while @rbuf.bytesize < 2 && __encoding__.name.start_with?("UTF-16") && __fill__(1)
    nil while __char_cut_short__ && __fill__(1)
    length = __char_length__
    nil while @rbuf.bytesize < length && __fill__(length - @rbuf.bytesize)
    length = @rbuf.bytesize if length > @rbuf.bytesize
    char = __take__(length).force_encoding(__encoding__)
    unless char.valid_encoding?
      # A byte to replace: CRuby's converter is given all that is buffered
      # and does not stop for a replacement, so `pos` is past what it went on
      # to (#609). From here the IO reads through it, as after such a line.
      if __replaces_invalid__?
        __stream_from__(char, @rquote)
        return getc
      end
      return __hand_out__(__broken_char__(char))
    end
    # So with `undef: :replace` is a character the internal encoding has no
    # character for, through a converter of more than one transcoder: it goes
    # on past the replacement too, where one transcoder stops (#634). Telling
    # an undefined character costs a conversion, so only where it is set.
    if @ecopts[:undef] == :replace && !@enc2.nil? && __xml_in__.nil? && __undefined_in_read__?(char)
      __stream_from__(char, @rquote)
      return getc
    end
    # Without an internal encoding CRuby's converter is the decorators alone,
    # which go a byte at a time, and the first output it stops after is an
    # attribute's quote with the first byte: the rest is still to be read,
    # for `pos` and for a `reopen` that keeps the converter (#593). Measured.
    if char.bytesize > 1 && !@rquote && @enc2.nil? && __xml_in__ == :attr
      @rbuf = char.byteslice(1, char.bytesize - 1).b << @rbuf
      char = char.byteslice(0, 1)
    end
    # With an internal encoding CRuby's first read stops after the
    # converter's first output, the attribute's quote. Into an encoding that
    # is not ASCII-compatible the decorators come before the transcoder, on
    # the UTF-8 it is read from, so that output is the quote and the first
    # byte: the converter holds the rest of what it took, which `pos` drops
    # with it, and the bytes after are read again without it (#633). Measured.
    if char.bytesize > 1 && !@rquote && !@enc2.nil? && __xml_in__ == :attr && __encoding__ == Encoding::UTF_8 && !__read_encoding__.ascii_compatible?
      @rbuf = char.byteslice(1, char.bytesize - 1).b << @rbuf
      @rpend = char.byteslice(0, 1).b
      @rquote = true
      return __hand_out__(__in_read__("\""))
    end
    # Into one that is, they come after it, and a character it cannot convert
    # is inserted as a reference, which is no output to stop after: the read
    # goes on to the next it can, or to the end of what is buffered, or to a
    # CR the universal newline decorator would hold (#633).
    if !@rquote && !@enc2.nil? && __xml_in__ == :attr && __read_encoding__.ascii_compatible? && __undefined_in_read__?(char)
      while !@rbuf.empty? && !(__universal_newline__? && @rbuf.getbyte(0) == 13)
        more = @rbuf.byteslice(0, __char_length__).force_encoding(__encoding__)
        break unless more.valid_encoding?
        char = (char.b << __take__(more.bytesize)).force_encoding(__encoding__)
        break unless __undefined_in_read__?(more)
      end
    end
    # The decorators alone hold a CR they reach until a later read sees the
    # byte after it: this read takes the CR and makes only the quote, and
    # `pos` is past the CR alone (#624). Measured.
    if !pended && @enc2.nil? && __xml_in__ == :attr && __universal_newline__? && char.b == "\r".b
      @rpend = char.b
      return __hand_out__(@rquote ? "" : (@rquote = true; __in_read__("\"")))
    end
    char, cr = __newline_char_in__(char)
    out = __decode__(char)
    # A replacement past a lone CR is all this read makes: the CR is held
    # again, for the next to tell (#626).
    if cr && (before = __read_ahead__(cr, cr.bytesize))
      __hold_chars__(before.b)
      return __take_char__
    end
    __hand_out__(out)
  end

  # Whether the read converter cannot convert `char` into the encoding reads
  # answer in, and so inserts what stands for it instead.
  def __undefined_in_read__?(char)
    char.encode(__read_encoding__)
    false
  rescue Encoding::UndefinedConversionError
    true
  rescue EncodingError
    false
  end

  # One character of what the read converter made for one read: `xml:`
  # makes an entity of `<`, and CRuby's `getc` hands it out a character at
  # a time, the rest held (#468). Measured.
  def __hand_out__(out)
    return out unless __xml_in__ || __chars_read__?
    __hold_chars__(out.b)
    __chars_held__? ? __take_char__ : getc
  end

  # Universal newline on a character read alone: a CR is an LF, and takes
  # the LF after it with it. CRuby waits for the character that says which,
  # on a pipe as well, and reads it: a lone CR answers its own bytes too, for
  # the character after it to be held. Measured.
  def __newline_char_in__(char)
    return char unless __universal_newline__?
    enc = char.encoding
    cr, lf = enc.ascii_compatible? ? ["\r".b, "\n".b] : ["\r".encode(enc).b, "\n".encode(enc).b]
    return char unless char.b == cr
    nil while @rbuf.bytesize < lf.bytesize && __fill__(lf.bytesize - @rbuf.bytesize)
    lone = @rbuf.byteslice(0, lf.bytesize) != lf
    __take__(lf.bytesize) unless lone
    [lf.force_encoding(enc), lone ? cr : nil]
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
    begin
      __decode__(char)
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
      __byte_readable__ unless __read_converter__?
    end
    self
  end

  # With a read converter the character goes back among the ones it made,
  # as it is, not to be converted again: CRuby's `rb_io_ungetc` puts it in
  # the `cbuf`, so a CR stays a CR with universal newline (#470). Measured.
  # Without one a String goes back as its bytes, as CRuby's
  # `io_ungetbyte`, in whatever encoding it is (#473). Measured.
  def ungetc(char)
    __readable__
    bytes = Integer === char ? (char.chr(__read_encoding__) rescue (char & 0xff).chr) : IO.__str__(char)
    __readconv__
    if __read_converter__?
      __hold_chars__(bytes.b)
    else
      @rbuf = bytes.b + @rbuf
    end
    nil
  end

  def ungetbyte(byte)
    __readable__
    __byte_readable__
    return nil if byte.nil?
    bytes = Integer === byte ? (byte & 0xff).chr : (String === byte ? byte : byte.to_str)
    @rbuf = bytes.b + @rbuf
    nil
  end

  def eof?
    __readable__
    !__chars_held__? && @rbuf.empty? && !__fill__
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
    # What writes in progress have gathered is written, and bytes buffered are
    # given back, first, as CRuby's `flush_before_seek`, and the converters
    # with them, even if the seek then fails; with none, they stay. Measured.
    __flush_gathered__
    __unread__(true)
    IO.__check__(__fd_seek__(@fileno, offset, whence), @path)
    0
  end

  def __whence__(whence)
    return whence unless Symbol === whence
    { SET: IO::SEEK_SET, CUR: IO::SEEK_CUR, END: IO::SEEK_END, DATA: IO::SEEK_DATA, HOLE: IO::SEEK_HOLE }.fetch(whence) do
      raise ArgumentError, "unknown whence: #{whence}"
    end
  end

  # CRuby's `rb_io_sysseek` refuses bytes read ahead, and only warns of a write
  # buffer holding bytes, here what writes in progress have gathered.
  def sysseek(offset, whence = IO::SEEK_SET)
    __check_open__
    raise IOError, "sysseek for buffered IO" unless @rbuf.empty? && !__chars_held__?
    warn("sysseek for buffered IO", uplevel: 1) if @wgathered&.any? { |text| !text.empty? }
    IO.__check__(__fd_seek__(@fileno, IO.__c_long__(offset), __whence__(whence)), @path)[0]
  end

  # `rewind` drops the read converter as CRuby's does, where `seek` drops it
  # only with bytes buffered. The characters it held count until a read of
  # characters makes another. Measured.
  def rewind
    seek(0)
    __drop_readconv__
    @lineno = 0
    0
  end

  # CRuby gives the bytes buffered back to find where the reader is, and
  # drops the converters with them, as `seek` does: what `ungetc` put there
  # goes too, unless that would seek before the start, and then they all
  # stay and count. What writes in progress have gathered is written first, as
  # `flush_before_seek` flushes the write buffer. Measured.
  def pos
    __check_open__
    __flush_gathered__
    __unread__
    IO.__check__(__fd_seek__(@fileno, 0, IO::SEEK_CUR), @path)[0] - @rbuf.bytesize
  end
  alias tell pos

  def pos=(position)
    seek(position)
    position
  end

  # --- writing ---------------------------------------------------------------

  # CRuby's `io_write_m`. One object is `io_write`, which makes it a String
  # before it looks at the IO, and for an empty one looks no further. Any other
  # number is `io_writev`, which checks the IO first, and an empty String goes
  # through the converter like another, so that makes one, an attribute's quote
  # and all. Measured.
  def write(*objects)
    if objects.size == 1
      text = objects[0]
      text = text.to_s unless String === text
      return 0 if text.empty?
      __write_side__
      return __write_all__(__encode_out__(text))
    end
    __write_side__
    __write_gathered__(objects)
  end

  # CRuby's `io_writev` walks the objects, and each one meeting a sync IO or a
  # tty takes all the rest to `writev`, one system call; before that they go to
  # the write buffer. So `text` is that buffer, listed in `@wgathered` while a
  # `to_s` can run, one for each write in progress, so a `close` or a write there
  # writes what came before, and `rest` is for `writev`. What comes after a close
  # goes in the buffer with no descriptor to reach, and counts. So does `rest`
  # if the buffer holds any, or if there are no bytes for `writev` to send; any
  # other `rest` finds the stream closed. So does the last object if it brought
  # the close with sync on, as `io_fwrite` tests sync after its `to_s`, and has
  # bytes. A tty is found only when a `to_s` flushes, where the list goes, so its
  # text is `rest`. A `flush`, `fsync`, `seek`, `pos`, `dup`, `truncate`,
  # `reopen` with a path, a read, `File#size` or `File#flock` there writes it
  # too, as each flushes CRuby's buffer, and `sysseek` warns of it. Measured.
  def __write_gathered__(objects)
    if @sync
      rest = __gather__(objects, 0)
      return 0 if rest.empty?
      raise IOError, "closed stream" if @closed || @wclosed
      return __write_all__(rest)
    end
    text = "".b
    rest = nil
    piece = nil
    total = 0
    index = 0
    (@wgathered ||= []) << text
    converted = false
    begin
      while index < objects.size
        if @wclosed ? @wsynced : @sync
          rest = __gather__(objects, index)
          total += rest.bytesize
          break
        end
        object = objects[index]
        object = object.to_s unless String === object
        piece = __encode_out__(object).b
        total += piece.bytesize
        text << piece
        index += 1
      end
      converted = true
    ensure
      flushed = @wgathered&.reject! { |listed| listed.equal?(text) }
      __write_partial__(text) unless converted
    end
    if @closed || @wclosed
      unsent = flushed ? (rest ? text.empty? && !rest.empty? : @wsynced && !piece.empty?) : total > 0
      raise IOError, "closed stream" if unsent
      return total
    end
    if rest
      text.empty? ? text = rest : text << rest
    end
    __write_all__(text) unless text.empty?
    total
  end

  # The objects from `index` on, as bytes for one `writev`.
  def __gather__(objects, index)
    text = "".b
    while index < objects.size
      object = objects[index]
      object = object.to_s unless String === object
      text << __encode_out__(object).b
      index += 1
    end
    text
  end

  def __write_partial__(text)
    __write_all__(text) unless @wclosed || text.empty? || tty?
  rescue StandardError
    nil
  end

  def syswrite(object)
    __write_side__
    text = String === object ? object : object.to_s
    IO.__check__(__fd_write__(__wfd__, text), @path)[0]
  end

  def write_nonblock(object, exception: true)
    __write_side__
    IO.__exception_flag__(exception)
    __flush_gathered__
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
    __write_side__
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
  # `[...]` where it recurs rather than looping. One write for each line, as
  # CRuby's `rb_io_puts`, so the lines before one that raises are written, on
  # a sync IO as well, where one `write` of them all would drop them (#420).
  def puts(*lines)
    if lines.empty?
      write(DEFAULT_RS)
      return nil
    end
    __puts_lines__(lines, [])
    nil
  end

  # --- another descriptor ----------------------------------------------------

  # CRuby's `rb_io_reopen`, which looks for an IO only in a lone argument.
  def reopen(other = nil, mode = (lone = true; nil), **options)
    if lone
      if !(IO === other) && other.respond_to?(:to_io)
        converted = other.to_io
        unless IO === converted
          raise TypeError, "can't convert #{other.class} to IO (#{other.class}#to_io gives #{converted.class})"
        end
        other = converted
      end
      return __reopen_io__(other) if IO === other
    end
    path = File.__path__(other)
    if mode.nil? && options.empty?
      # Everything stays, the mode's creating and truncating among it.
      # Neither reading nor writing opens as a reader does.
      flags = (@access == 3 ? File::RDONLY : @access) | (@oflags & (File::CREAT | File::TRUNC | File::APPEND | File::EXCL))
    else
      # The mode starts again, and with it `sync` and `autoclose`, but no BOM
      # is read past: only opening a file does that. `lineno` stays.
      __modeenc__(mode, options) { |flags, binmode, ecopts| __check_access__(flags, binmode, ecopts) }
      flags = @oflags
      @sync = false
      @autoclose = true
      @duplex = nil
      @rmoded = true
    end
    # What writes in progress have gathered is written if the IO now writes,
    # as `rb_io_reopen` flushes by the new mode. The bytes buffered go, even if
    # what is named does not open; the read converter and the characters it
    # holds stay, whatever was opened.
    __flush_gathered__ if !@closed && __writes__?
    @rbuf = "".b
    __keep_readconv__
    fd = IO.__check__(__fd_open__(path, flags, 0666), path, "rb_sysopen")[0]
    __untie__
    if @closed
      @fileno = fd
      @closed = false
    else
      IO.__check__(__fd_dup2__(fd, @fileno), path)
      __fd_close__(fd)
    end
    @path = path.dup
    __arm__
    self
  end

  # A duplex child whose write end is closed is one IO from here on, as CRuby's
  # is once `close_write` unties it; what `close` left tied stays in `@wgone`.
  def __untie__
    return if @wfileno.nil? || !@wclosed
    @wfileno = nil
    @wclosed = false
    @rclosed = false
    @finalizer&.wfd = nil
  end

  # CRuby's `io_reopen`. A reader's bytes buffered go, and the read converter
  # with them, where there were any; with none it stays, for reads to go on
  # through (`__keep_readconv__`). What the descriptor is at then decides what
  # else goes. The other IO gives back its own, and this one takes all it is
  # but `autoclose`, which either can turn off, and what its first write fixed,
  # which stays for its writes while other reads go by what it took. A binary
  # IO takes the newlines out of that too, as `io_reopen` ends in
  # `rb_io_binmode`. Measured.
  def __reopen_io__(other)
    other.__check_open__
    __check_open__
    return self if other.equal?(self)
    # Only here is it `stdio_file` that counts: an `autoclose = false` IO can
    # take any access from an IO, and a standard stream only one it has.
    if @stdio && !@autoclose && other.__side__ != 2 && other.__side__ != (@fileno == 0 ? 0 : 1)
      __refuse_access__(other.__modestr__)
    end
    __unread__(true) if @access == 0
    __untie__
    position = other.__seekable_pos__ if other.__reads__?
    @access, @oflags, @binmode, @textmode, @sync, @enc, @enc2, @ecopts, @lineno, @pid, @duplex = other.__modes__
    @rmoded = !other.__tied__? || other.__rmoded__
    @wpre = @wpre.reject { |key, _| %i[newline universal_newline crlf_newline cr_newline lf_newline].include?(key) } if @wpre && @binmode
    @autoclose &&= other.autoclose?
    @finalizer&.autoclose = @autoclose
    @path = other.path || (@path unless @autoclose)
    unless other.fileno == @fileno
      IO.__check__(__fd_dup2__(other.fileno, @fileno), @path)
      unless position.nil?
        __unread__(true)
        IO.__check__(__fd_seek__(@fileno, position, IO::SEEK_SET))
      end
    end
    __keep_readconv__
    self
  end

  # 0 reads, 1 writes, 2 does both and 3 does neither, which a closed IO is
  # and stays when `reopen` gives it a path and no mode.
  def __reads__? = @access == 0 || @access == 2

  def __writes__? = @access == 1 || @access == 2

  # CRuby's `rb_io_reopen`: an IO that is `RUBY_IO_EXTERNAL_P`, a standard
  # stream or one with `autoclose = false`, keeps the access it has. What it
  # does not have it may take. Asked with a mode that is about to be assigned.
  def __check_access__(flags, binmode, ecopts)
    access = flags & 3
    return if @autoclose || access == 2 || __side__ == 3 || __side__ == access
    __refuse_access__(IO.__modestr__(access, flags, binmode, __newline_decorator__?(ecopts)))
  end

  # ponytail: CRuby takes the path for the name and crashes on an IO without
  # one, a pipe or `IO.for_fd`, which here says its descriptor instead.
  def __refuse_access__(mode)
    raise ArgumentError, "#{@path || "fd #{@fileno}"} can't change access mode from #{__modestr__.inspect} to #{mode.inspect}"
  end

  # A duplex child's access as a mode counts it: CRuby makes that two IOs, and
  # this one is the side that reads.
  def __side__ = @access != 3 && __tied__? ? 0 : @access

  def __modestr__ = IO.__modestr__(__side__, @oflags, @binmode, __newline_decorator__?)

  # What a mode is called, CRuby's `rb_io_fmode_modestr`.
  # The `t` is a newline decorator's: what was asked for without one is lost.
  def self.__modestr__(access, flags, binmode, text)
    kind = binmode ? "b" : (text ? "t" : "")
    both = access == 2 ? "+" : ""
    excl = flags & File::EXCL == 0 ? "" : "x"
    return "a#{kind}#{both}" unless flags & File::APPEND == 0
    if access == 3
      # ponytail: the bits a mode names, without those of what the IO became.
      bits = { File::CREAT => 0x80, File::EXCL => 0x400, File::TRUNC => 0x800 }
      fmode = bits.sum { |flag, bit| flags & flag == 0 ? 0 : bit }
      fmode |= binmode ? 0x4 : (text ? 0x1000 : 0)
      raise ArgumentError, "invalid access fmode 0x#{fmode.to_s(16)}"
    end
    return "r#{kind}" if access == 0
    return "r#{kind}+" if access == 2 && flags & File::CREAT == 0
    "w#{kind}#{both}#{excl}"
  end

  # What only `STDIN`, `STDOUT` and `STDERR` are, until they are closed.
  def __stdio__
    @stdio = true
    self
  end

  # What `reopen` takes from this IO.
  def __modes__
    [@access, @oflags, @binmode, @textmode, @sync, @enc, @enc2, @ecopts.dup, @lineno, @pid, @duplex]
  end

  # Where the descriptor is, once it has given back what is buffered.
  def __seekable_pos__
    __unread__
    position = __fd_seek__(@fileno, 0, IO::SEEK_CUR)
    Integer === position ? nil : position[0]
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
    flags, = IO.__mode_flags__(mode)
    access = flags & 3
    io_keys = %i[external_encoding internal_encoding encoding textmode binmode autoclose invalid undef replace newline universal_newline crlf_newline cr_newline lf_newline xml]
    io_options = options.select { |key, _| io_keys.include?(key) }
    # A key `spawn` does not know is left for the IO, as `rb_execarg_extract_options`
    # leaves it, where it is ignored; in the command it is still refused.
    spawn_options = command_options.merge(options.select { |key, _| Process::Spawn.option?(key) })
    # Read before the child starts, as `rb_io_popen` does, in the mode as it
    # was given. A pipe has no BOM read past, however the mode names one.
    io = allocate
    io.__modeenc__(mode, io_options)
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
    io.__open_fd__(access == 1 ? parent_w : parent_r, io_options, false)
    io.__pid__(pid, access == 2 ? parent_w : nil)
    io.sync = true if access != 0
    return io unless block
    begin
      yield io
    ensure
      io.close unless io.closed?
    end
  ensure
    [parent_r, parent_w].each { |fd| __fd_close__(fd) unless fd.nil? } if pid.nil?
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

  # From one IO or file to another, a block at a time, as CRuby's
  # `copy_stream_body`. Each end is an IO, one a String or a `to_path` names,
  # or else a stream, with its own `read` and `write`. Between two IOs the bytes
  # go by the descriptors, and those methods are not called. A source offset
  # reads with `pread` and leaves the source where it was.
  def self.copy_stream(source, destination, length = nil, offset = nil)
    length = Integer.__index__(length) unless length.nil?
    position = offset.nil? ? nil : Integer.__index__(offset)
    # Measured: a negative one is as good as none.
    length = nil if length&.negative?
    position = nil if position&.negative?
    close_source = close_destination = false
    copied = 0
    begin
      from_file = __copy_stream_file__?(source)
      if from_file
        # What `to_io` gives, or the file named, and never its own `read`.
        source = IO.try_convert(source) || source
        unless IO === source
          source = File.open(File.__path__(source), "rb")
          close_source = true
        end
        source.__readable__
        source.__byte_readable__
      end
      to_file = __copy_stream_file__?(destination)
      if to_file
        destination = IO.try_convert(destination) || destination
        unless IO === destination
          destination = File.open(File.__path__(destination), "wb")
          close_destination = true
        end
        # An IO stays binary after the copy, whatever it was, even for no
        # bytes at all. It drops the converters unfinished, and the bytes go
        # out as they are.
        destination.__write_side__
        destination.__write_binmode__
      end
      # What the source has read ahead goes first, and counts whatever a
      # stream's `write` answers.
      if from_file && position.nil?
        held = source.__take__(length || source.__held_bytes__)
        unless held.empty?
          to_file ? destination.__write_all__(held) : destination.__send__(:write, held)
          copied = held.bytesize
        end
      end
      rest = length.nil? ? nil : length - copied
      unless rest == 0
        copied += if from_file && to_file
                    __copy_stream_bytes__(source, destination, rest, position)
                  else
                    __copy_stream_fallback__(source, destination, from_file, rest, position)
                  end
      end
    ensure
      source.close if close_source
      destination.close if close_destination
    end
    copied
  end

  # Whether `copy_stream` takes this end as an IO or a file.
  def self.__copy_stream_file__?(stream)
    IO === stream || String === stream || __copy_stream_answers__?(stream, :to_path)
  end

  # Whether a public `name` is there, as CRuby's `rb_respond_to`. A
  # `respond_to?` of its own is asked, whatever its visibility; otherwise, and
  # for a `BasicObject`, which has none, the object is looked at.
  def self.__copy_stream_answers__?(stream, name)
    if Kernel.__overrides__?(__reflect_class_of__(stream), :respond_to?, Kernel)
      stream.__send__(:respond_to?, name)
    else
      Kernel.instance_method(:respond_to?).bind(stream).call(name)
    end
  end

  # CRuby's `nogvl_copy_stream_func`: descriptor to descriptor. From a file
  # with no offset that is `copy_file_range` or else `sendfile`, which copy what
  # the file holds past where the source is, reading from the source's place
  # and writing at the destination's, both as they were before. So one IO at
  # both ends, or two that share a place, writes the bytes back where they were
  # read (#536). Here those are `pread`s from the source's place, and then the
  # source is moved past them, unless the writes have already moved it.
  def self.__copy_stream_bytes__(source, destination, length, position)
    if position.nil?
      stat = source.__stat__
      return __copy_stream_chunks__(source, destination, length, nil) unless stat.file?
      start = source.__fd_pos__
      length = stat.size > start ? stat.size - start : 0 if length.nil?
      copied = 0
      begin
        copied = __copy_stream_chunks__(source, destination, length, start)
      ensure
        source.__fd_pos__ = start + copied if copied > 0 && source.__fd_pos__ == start
      end
      copied
    else
      __copy_stream_chunks__(source, destination, length, position)
    end
  end

  def self.__copy_stream_chunks__(source, destination, length, position)
    copied = 0
    until copied == length
      want = length.nil? || length - copied > 65536 ? 65536 : length - copied
      chunk = if position.nil?
                source.__partial__(want, nil, "readpartial")
              else
                # Measured: an `Errno::ESPIPE` names the call.
                source.__pread_bytes__(want, position + copied, "pread")
              end
      break if chunk.nil? || chunk.empty?
      destination.__write_side__
      destination.__write_all__(chunk)
      copied += chunk.bytesize
    end
    copied
  end

  # CRuby's `copy_stream_fallback`, when one end is a stream: one buffer, handed
  # to `readpartial` or else `read` and then to `write`, and what `write`
  # answers is what counts. Those are called whatever their visibility, and an
  # `EOFError` from either ends the copy.
  def self.__copy_stream_fallback__(source, destination, from_file, length, position)
    raise ArgumentError, "cannot specify src_offset for non-IO" unless from_file || position.nil?
    reader = __copy_stream_answers__?(source, :readpartial) ? :readpartial : :read unless from_file
    buffer = "".b
    rest = length
    copied = 0
    begin
      until rest == 0
        want = rest.nil? || rest > 16384 ? 16384 : rest
        if !from_file
          break if source.__send__(reader, want, buffer).nil? && reader == :read
        elsif position.nil?
          break if source.__partial__(want, buffer, "readpartial").nil?
        else
          chunk = source.__pread_bytes__(want, position, "pread")
          break if chunk.empty?
          buffer.replace(chunk)
          position += chunk.bytesize
        end
        written = Integer.__index__(destination.__send__(:write, buffer))
        copied += written
        rest -= written unless rest.nil?
        break if reader == :read && buffer.empty?
      end
    rescue EOFError
      nil
    end
    copied
  end
end

# The standard streams. Made without `new`, which checks the descriptor: a
# process may start with one of them closed.
# Measured: `autoclose?` is false, and `close` leaves the descriptor open.
STDIN = IO.allocate.__setup__(0, "r", { path: "<STDIN>", autoclose: false }).__stdio__
STDOUT = IO.allocate.__setup__(1, "w", { path: "<STDOUT>", autoclose: false }).__stdio__
STDERR = IO.allocate.__setup__(2, "w", { path: "<STDERR>", autoclose: false }).__stdio__
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
# The input record separator: what `gets` and `each_line` split on. It
# starts as CRuby's `rb_default_rs`, a frozen US-ASCII newline, and
# assigning one equal to it puts that very object back (#484).
class IO
  DEFAULT_RS = "\n".dup.force_encoding(Encoding::US_ASCII).freeze
  private_constant :DEFAULT_RS

  def self.__default_rs__
    DEFAULT_RS
  end
end
$/ = IO.__default_rs__
alias $-0 $/

module Kernel
  # CRuby's `rb_f_puts`: on `$stdout` itself, a plain object whose only
  # method is `write`, the lines go out as `IO#puts` writes them (#574);
  # anything else is asked for its own `puts`, private or not.
  def puts(*lines)
    out = $stdout
    return out.__send__(:puts, *lines) unless equal?(out)
    if lines.empty?
      write(IO.__default_rs__)
      return nil
    end
    __puts_lines__(lines, [])
    nil
  end

  # Several pieces in one `write`, as CRuby's `rb_io_writev`; a `write` that
  # takes one argument, the interface before Ruby 2.5, gets them one at a time.
  def __writev__(pieces)
    if pieces.size > 1 && respond_to?(:write, true) && method(:write).arity == 1
      pieces.each { |piece| write(piece) }
    else
      write(*pieces)
    end
  end

  def __puts_lines__(lines, seen)
    lines.each do |line|
      if String === line
        __puts_line__(line)
        next
      end
      array = Array === line ? line : (line.respond_to?(:to_ary, true) ? line.to_ary : nil)
      if Array === array
        if seen.any? { |outer| outer.equal?(array) }
          __puts_line__("[...]".b)
        else
          __puts_lines__(array, seen + [array])
        end
      else
        text = line.nil? ? "" : line.to_s
        text = Kernel.instance_method(:to_s).bind_call(line) unless String === text
        __puts_line__(text)
      end
    end
  end

  def __puts_line__(text)
    # An empty line writes only the newline, as CRuby's `io_puts_ary` does.
    __writev__(text.__end_with_newline__ ? [text] : (text.empty? ? [IO.__default_rs__] : [text, IO.__default_rs__]))
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
