# `io/console` (#110), as CRuby 4.0 ships it: io-console 0.8.2. CRuby's is
# `ext/io/console/console.c`, a C extension; here it is Ruby over the terminal
# calls in `spinel-vm/src/console.rs`, the `__console_*__` primitives:
# `tcgetattr` and `tcsetattr`, the window size `ioctl`s, `tcflush` and
# `ttyname_r`. What each method answers, raises and writes follows
# `console.c` on a termios platform, measured against CRuby 4.0.9 on Linux.
#
# A terminal's mode is `[iflag, oflag, cflag, lflag, cc, ispeed, ospeed]`, `cc`
# a BINARY String of the control characters, and is never changed in place:
# each setter answers a new one.

class IO
  class ConsoleMode
    # `rb_str_new_cstr`, so BINARY, and frozen.
    VERSION = "0.8.2".b.freeze

    # The termios flags, control character indices and queue selectors.
    @__flags__ = __console_constants__.to_h.freeze

    def self.__flag__(name) = @__flags__.fetch(name)

    # `cfmakeraw`, then what `set_rawmode` does past it: echo erasing off,
    # the read's minimum count and timeout, and with `intr:` the signals.
    def self.__raw__(mode, options)
      f = @__flags__
      iflag, oflag, cflag, lflag, cc, ispeed, ospeed = mode
      iflag &= ~(f["IGNBRK"] | f["BRKINT"] | f["PARMRK"] | f["ISTRIP"] |
                 f["INLCR"] | f["IGNCR"] | f["ICRNL"] | f["IXON"])
      oflag &= ~f["OPOST"]
      lflag &= ~(f["ECHO"] | f["ECHONL"] | f["ICANON"] | f["ISIG"] | f["IEXTEN"])
      cflag = (cflag & ~(f["CSIZE"] | f["PARENB"])) | f["CS8"]
      lflag &= ~(f["ECHOE"] | f["ECHOK"])
      cc = cc.b
      vmin, vtime, intr = options
      # A `cc_t` is a byte: C's assignment keeps the low eight bits.
      cc.setbyte(f["VMIN"], vmin & 0xFF) if vmin >= 0
      cc.setbyte(f["VTIME"], vtime & 0xFF) if vtime >= 0
      if intr
        iflag |= f["BRKINT"]
        lflag |= f["ISIG"]
        oflag |= f["OPOST"]
      end
      [iflag, oflag, cflag, lflag, cc, ispeed, ospeed]
    end

    # `set_cookedmode`.
    def self.__cooked__(mode)
      f = @__flags__
      iflag, oflag, cflag, lflag, cc, ispeed, ospeed = mode
      iflag |= f["BRKINT"] | f["ISTRIP"] | f["ICRNL"] | f["IXON"]
      oflag |= f["OPOST"]
      lflag |= f["ECHO"] | f["ECHOE"] | f["ECHOK"] | f["ECHONL"] | f["ICANON"] |
               f["ISIG"] | f["IEXTEN"]
      [iflag, oflag, cflag, lflag, cc, ispeed, ospeed]
    end

    # `set_echo` and `set_noecho`.
    def self.__echo__(mode, on)
      f = @__flags__
      echo = f["ECHO"] | f["ECHOE"] | f["ECHOK"] | f["ECHONL"]
      lflag = on ? mode[3] | echo : mode[3] & ~echo
      [mode[0], mode[1], mode[2], lflag, *mode[4, 3]]
    end

    # `echo_p`.
    def self.__echo_p__(mode) = mode[3] & (@__flags__["ECHO"] | @__flags__["ECHONL"]) != 0

    # `rawmode_opt`'s keywords as `[vmin, vtime, intr]`, with `stty raw`'s
    # defaults: `NUM2INT` of `min:`, of `time: * 10`, and `intr:` true or false.
    def self.__raw_options__(min, time, intr)
      vmin = min.nil? ? 1 : IO.__c_int__(min)
      vtime = time.nil? ? 0 : IO.__c_int__(time.public_send(:*, 10))
      # `switch (intr)`: the objects themselves, not whatever `==` says.
      unless intr.nil? || true.equal?(intr) || false.equal?(intr)
        raise ArgumentError, "true or false expected as intr: #{intr}"
      end
      [vmin, vtime, true.equal?(intr)]
    end

    # All zeroes, as `conmode_alloc` leaves one.
    def self.__zero__
      @__zero__ ||= [0, 0, 0, 0, ("\0" * @__flags__["NCCS"]).b.freeze, 0, 0].freeze
    end

    # `rb_check_typeddata`'s refusal, naming the class as
    # `rb_builtin_class_name` does.
    def self.__check__(object)
      return object.__mode__ if ConsoleMode === object
      name = case object when nil, true, false then object.inspect else object.class end
      raise TypeError, "wrong argument type #{name} (expected console-mode)"
    end

    def self.__wrap__(mode)
      console = allocate
      console.__mode__ = mode
      console
    end

    undef_method :initialize

    def __mode__ = @__mode__ || ConsoleMode.__zero__

    def __mode__=(mode)
      mode[4] = mode[4].b.freeze
      @__mode__ = mode.freeze
    end

    def initialize_copy(other)
      self.__mode__ = ConsoleMode.__check__(other).dup
      self
    end

    def echo=(on)
      self.__mode__ = ConsoleMode.__echo__(__mode__, on)
    end

    def raw!(min: nil, time: nil, intr: nil)
      options = ConsoleMode.__raw_options__(min, time, intr)
      self.__mode__ = ConsoleMode.__raw__(__mode__, options)
      self
    end

    def raw(min: nil, time: nil, intr: nil)
      options = ConsoleMode.__raw_options__(min, time, intr)
      self.class.__wrap__(ConsoleMode.__raw__(__mode__, options))
    end
  end

  # --- the terminal's mode -----------------------------------------------------

  # `sys_fail`: the errno, with the IO's path when it has one.
  def __console_fail__(errno)
    raise SystemCallError.new(@path, errno)
  end

  # `GetWriteFD`: the descriptor writes go to, the IO checked open.
  def __console_wfd__
    __check_open__
    __wfd__
  end

  # `set_ttymode`: the mode of `fd` changed by `setter`; the mode it had, or
  # the errno of the call that failed.
  def __console_set__(fd, setter)
    mode = __console_getattr__(fd)
    return mode if Integer === mode
    __console_setattr__(fd, *setter.call(mode)) || mode
  end

  # `ttymode`: the read and write descriptors' modes changed by `setter` while
  # the block runs, and put back after, even when it raises. A descriptor that
  # is no terminal is left alone; when neither is one, nothing runs.
  def __console_ttymode__(setter)
    rfd = fileno
    rmode = __console_set__(rfd, setter)
    error = rmode if Integer === rmode
    wfd = __wfd__
    wmode = nil
    if wfd != rfd
      wmode = __console_set__(wfd, setter)
      error = wmode if Integer === wmode
    end
    changed = Array === rmode || Array === wmode
    begin
      result = yield if changed
    ensure
      failed = nil
      if Array === rmode && rfd == fileno
        failed = __console_setattr__(rfd, *rmode) || failed
      end
      if Array === wmode && wfd == __console_wfd__
        failed = __console_setattr__(wfd, *wmode) || failed
      end
      raise SystemCallError.new(nil, failed) if failed
    end
    raise SystemCallError.new(nil, error) unless changed
    result
  end

  # The read descriptor's mode, raising as `sys_fail` does.
  def __console_mode_of__(fd)
    mode = __console_getattr__(fd)
    __console_fail__(mode) if Integer === mode
    mode
  end

  # The read descriptor's mode changed by `setter`, for good.
  def __console_change__(&setter)
    fd = fileno
    errno = __console_setattr__(fd, *setter.call(__console_mode_of__(fd)))
    __console_fail__(errno) if errno
    self
  end

  # `rb_yield`, which with no block raises once the mode is set, as CRuby's
  # does, and says only "no block given".
  def __console_yield__(block)
    raise LocalJumpError, "no block given" if block.nil?
    block.call(self)
  end

  def raw(min: nil, time: nil, intr: nil, &block)
    options = ConsoleMode.__raw_options__(min, time, intr)
    __console_ttymode__(->(mode) { ConsoleMode.__raw__(mode, options) }) { __console_yield__(block) }
  end

  def raw!(min: nil, time: nil, intr: nil)
    options = ConsoleMode.__raw_options__(min, time, intr)
    __console_change__ { |mode| ConsoleMode.__raw__(mode, options) }
  end

  def cooked(&block)
    __console_ttymode__(->(mode) { ConsoleMode.__cooked__(mode) }) { __console_yield__(block) }
  end

  def cooked!
    __console_change__ { |mode| ConsoleMode.__cooked__(mode) }
  end

  def getch(min: nil, time: nil, intr: nil)
    options = ConsoleMode.__raw_options__(min, time, intr)
    __console_ttymode__(->(mode) { ConsoleMode.__raw__(mode, options) }) { getc }
  end

  def noecho(&block)
    __console_ttymode__(->(mode) { ConsoleMode.__echo__(mode, false) }) { __console_yield__(block) }
  end

  def echo=(on)
    __console_change__ { |mode| ConsoleMode.__echo__(mode, on) }
  end

  def echo?
    ConsoleMode.__echo_p__(__console_mode_of__(fileno))
  end

  def console_mode
    ConsoleMode.__wrap__(__console_mode_of__(fileno).dup)
  end

  def console_mode=(mode)
    fd = fileno
    errno = __console_setattr__(fd, *ConsoleMode.__check__(mode))
    __console_fail__(errno) if errno
    mode
  end

  # --- the window --------------------------------------------------------------

  def winsize
    size = __console_winsize__(__console_wfd__)
    __console_fail__(size) if Integer === size
    size.first(2)
  end

  def winsize=(size)
    size = Array(size)
    unless size.length == 2 || size.length == 4
      raise ArgumentError, "wrong number of arguments (given #{size.length}, expected 2 or 4)"
    end
    # Each `NUM2UINT`, kept to an unsigned short as `console.c`'s cast does.
    fields = [size[0], size[1], size[2], size[3]].map { |n| n.nil? ? 0 : IO.__c_uint__(n) & 0xFFFF }
    errno = __console_set_winsize__(__console_wfd__, *fields)
    __console_fail__(errno) if errno
    self
  end

  def iflush
    errno = __console_flush__(fileno, ConsoleMode.__flag__("TCIFLUSH"))
    __console_fail__(errno) if errno
    self
  end

  def oflush
    errno = __console_flush__(__console_wfd__, ConsoleMode.__flag__("TCOFLUSH"))
    __console_fail__(errno) if errno
    self
  end

  def ioflush
    rfd = fileno
    wfd = __console_wfd__
    errno =
      if wfd != rfd
        __console_flush__(rfd, ConsoleMode.__flag__("TCIFLUSH")) ||
          __console_flush__(wfd, ConsoleMode.__flag__("TCOFLUSH"))
      else
        __console_flush__(rfd, ConsoleMode.__flag__("TCIOFLUSH"))
      end
    __console_fail__(errno) if errno
    self
  end

  def beep
    written = __fd_write__(__console_wfd__, "\a")
    __console_fail__(written) if Integer === written
    self
  end

  # --- the cursor and the screen, as VT100 sequences -------------------------

  # `%d` of an `unsigned int` plus one, as C prints it: wrapped, then signed.
  def self.__console_column__(n)
    n = (IO.__c_uint__(n) + 1) & 0xFFFF_FFFF
    n >= 0x8000_0000 ? n - 0x1_0000_0000 : n
  end

  def goto(y, x)
    write("\e[#{IO.__console_column__(y)};#{IO.__console_column__(x)}H")
    self
  end

  def cursor
    raw = ->(mode) { ConsoleMode.__raw__(mode, [1, 0, false]) }
    response = __console_ttymode__(raw) do
      write("\e[6n")
      flush
      __console_vt_response__
    end
    return nil unless Array === response && response.length == 3
    return nil unless response[2] == "R"
    [IO.__console_row__(response[0]), IO.__console_row__(response[1])]
  end

  # `NUM2UINT(n) - 1` as an `unsigned int`, and that given back as an `INT2NUM`.
  def self.__console_row__(n)
    n = (n - 1) & 0xFFFF_FFFF
    n >= 0x8000_0000 ? n - 0x1_0000_0000 : n
  end

  # `read_vt_response`: `ESC [`, then numbers split by `;`, then the final
  # character; nil for anything else.
  def __console_vt_response__
    return nil unless getbyte == 0x1b
    return nil unless getbyte == 0x5b
    result = []
    number = 0
    last = nil
    while (byte = getbyte)
      if byte == 0x3b
        result << number
        number = 0
      elsif byte >= 0x30 && byte <= 0x39
        number = number * 10 + byte - 0x30
      else
        result << number
        last = byte.chr
        break
      end
    end
    result << last
  end

  def cursor=(position)
    position = Array.__coerce__(position)
    raise ArgumentError, "expected 2D coordinate" unless position.length == 2
    goto(position[0], position[1])
  end

  # `console_move`: up or down, then left or right, written and flushed.
  def __console_move__(y, x)
    return self if y.zero? && x.zero?
    sequence = +""
    sequence << "\e[#{y.abs}#{y < 0 ? "A" : "B"}" unless y.zero?
    sequence << "\e[#{x.abs}#{x < 0 ? "D" : "C"}" unless x.zero?
    write(sequence)
    flush
    self
  end

  def cursor_up(n) = __console_move__(-IO.__c_int__(n), 0)
  def cursor_down(n) = __console_move__(IO.__c_int__(n), 0)
  def cursor_left(n) = __console_move__(0, -IO.__c_int__(n))
  def cursor_right(n) = __console_move__(0, IO.__c_int__(n))

  def goto_column(column)
    write("\e[#{IO.__console_column__(column)}G")
    self
  end

  # `mode_in_range`: nil is 0; anything but an Integer from 0 to `high` is
  # refused, by its `to_s`, and one past a C int by `NUM2INT` first.
  def self.__console_erase_mode__(mode, high, name)
    return 0 if mode.nil?
    raise ArgumentError, "wrong #{name} mode: #{mode}" unless Integer === mode
    unless IO.__c_int__(mode) >= 0 && mode <= high
      raise ArgumentError, "wrong #{name} mode: #{mode}"
    end
    mode
  end

  def erase_line(mode)
    write("\e[#{IO.__console_erase_mode__(mode, 2, "line erase")}K")
    self
  end

  def erase_screen(mode)
    write("\e[#{IO.__console_erase_mode__(mode, 3, "screen erase")}J")
    self
  end

  # `console_scroll`: up for a positive count, down for a negative one.
  def __console_scroll__(lines)
    write("\e[#{lines.abs}#{lines < 0 ? "T" : "S"}") unless lines.zero?
    self
  end

  def scroll_forward(n) = __console_scroll__(IO.__c_int__(n))
  def scroll_backward(n) = __console_scroll__(-IO.__c_int__(n))

  def clear_screen
    erase_screen(2)
    goto(0, 0)
  end

  # Windows only: `rb_f_notimplement` everywhere else.
  def pressed?(_key)
    raise NotImplementedError, "pressed?() function is unimplemented on this machine"
  end

  def check_winsize_changed
    raise NotImplementedError, "check_winsize_changed() function is unimplemented on this machine"
  end

  # --- reading a password -----------------------------------------------------

  def getpass(*args)
    unless args.length <= 1
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..1)"
    end
    out = self.equal?($stdin) ? $stderr : self
    IO.__console_prompt__(out, args[0])
    out.flush
    begin
      line = __console_ttymode__(->(mode) { ConsoleMode.__echo__(mode, false) }) { gets }
    ensure
      # `rb_default_rs`, which is "\n" whatever `$/` is.
      out.write("\n")
    end
    line&.chomp!("\n")
    line
  end

  # `prompt`: a String with no NUL written as it is.
  def self.__console_prompt__(io, prompt)
    return if prompt.nil?
    prompt = String.__coerce__(prompt)
    raise ArgumentError, "string contains null byte" if prompt.include?("\0")
    io.write(prompt)
  end

  def ttyname
    name = __console_ttyname__(fileno)
    raise SystemCallError.new("ttyname_r(#{fileno})", name) if Integer === name
    return nil if name.nil?
    # `rb_interned_str_cstr`: frozen, interned, US-ASCII.
    -name.force_encoding(Encoding::US_ASCII)
  end

  # --- the console --------------------------------------------------------------

  # `/dev/tty`, opened once per class and kept until it is closed; nil when
  # there is none. With a method name, that method sent to it.
  #
  # ponytail: kept in a hidden instance variable of the class, as io-console
  # does where it has no Ractor-local storage. CRuby 4.0 keeps it per Ractor;
  # once Ractors run (phase 5) a non-main one cannot write a shared class, so
  # this moves to Ractor-local storage then.
  def self.console(*args)
    unless args.empty? || Symbol === args[0]
      name = case args[0] when nil, true, false then args[0].inspect else args[0].class end
      raise TypeError, "wrong argument type #{name} (expected Symbol)"
    end
    klass = equal?(IO) ? File : self
    console = klass.instance_variable_get(:@__console__)
    if console && (!(IO === console) || console.closed?)
      klass.remove_instance_variable(:@__console__)
      console = nil
    end
    if args.length == 1 && args[0] == :close
      if console
        console.close
        klass.remove_instance_variable(:@__console__)
      end
      return nil
    end
    unless console
      begin
        fd = IO.sysopen("/dev/tty", File::RDWR)
      rescue SystemCallError
        return nil
      end
      console = klass.for_fd(fd, File::RDWR, path: "/dev/tty")
      console.__console_device__
      klass.instance_variable_set(:@__console__, console)
    end
    return console.__send__(*args) unless args.empty?
    console
  end

  # What `rb_io_open_descriptor` leaves the console: synchronized, and not yet
  # `FMODE_DUPLEX`, since it skips `io_check_tty`, so the first write after a
  # read ahead discards what was read, as `rb_io_check_writable` does.
  def __console_device__
    @sync = true
    @duplex = nil
  end

  # `IO::generic_readable`, which `stringio` includes: `getc` and `gets` for
  # an object that is no terminal.
  __generic_module__(:generic_readable).module_eval do
    def getch(*args, **options) = getc(*args, **options)

    def getpass(*args)
      unless args.length <= 1
        raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..1)"
      end
      IO.__console_prompt__(self, args[0])
      flush if respond_to?(:flush)
      begin
        line = gets
      ensure
        write("\n")
      end
      line&.chomp!("\n")
      line
    end
  end
end
