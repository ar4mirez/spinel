# `ARGF`, the stream of the files named in `ARGV`, one after another, or of
# standard input when there are none (#41): CRuby's `argf_*` functions in
# `io.c`, in Ruby.
#
# Its class has no constant: it is `ARGF.class`, named so as CRuby's
# `rb_set_class_path` names it. Its state is CRuby's `struct argf`, in hidden
# instance variables, so `ARGF.instance_variables` is empty as there:
#
# - `@__init_p__` is 0 before anything is opened, 1 once a stream is, and -1
#   just after one was closed;
# - `@__next_p__` is 1 when the next file in `argv` is to be opened, 0 while
#   one is being read, and -1 when the stream is standard input because
#   `argv` was empty.
#
# `$.` is the global `ARGF`'s `last_lineno`, which `IO#gets` sets too: a line
# read from the file `ARGF` is reading counts in `ARGF.lineno` as well, and
# one read from any other IO makes `$.` that IO's line number
# (`__io_line__`, CRuby's `rb_io_getline_1`).
#
# ponytail: in-place mode is not here yet. `inplace_mode`, `$-i`, `-i[ext]`
# and the writing methods (`write`, `print`, `putc`, `puts`, `printf`,
# `to_write_io`) that write to the file being edited are #41's next slice.
ARGF = Class.new do
  include Enumerable

  def initialize(*argv)
    @__filename__ = nil
    @__current_file__ = nil
    @__lineno__ = 0
    @__argv__ = argv
    @__encs__ = nil
    @__init_p__ = 0
    @__next_p__ = 0
    @__binmode__ = false
  end

  def initialize_copy(orig)
    return self if equal?(orig)
    super
    @__argv__ = @__argv__.dup
    self
  end

  def to_s = +"ARGF"
  alias inspect to_s

  def argv = @__argv__

  def lineno = @__lineno__

  def lineno=(value)
    @__lineno__ = ARGF.class.__c_int__(value)
    __global_store__(:$., @__lineno__) if equal?(ARGF)
    value
  end

  def filename
    __next_argv__
    @__filename__
  end
  alias path filename

  def file
    __next_argv__
    @__current_file__
  end

  def fileno
    raise ArgumentError, "no stream" unless __next_argv__
    @__current_file__.fileno
  end
  alias to_i fileno

  def to_io
    __next_argv__
    return @__current_file__.to_io if __generic_input__
    @__current_file__
  end

  def gets(*args, **options)
    $_ = __getline__(args, options)
  end

  def readline(*args, **options)
    raise EOFError, "end of file reached" unless __next_argv__
    return @__current_file__.readline(*args, **options) if __generic_input__
    line = $_ = __getline__(args, options)
    raise EOFError, "end of file reached" if line.nil?
    line
  end

  def readlines(*args, **options)
    lineno = @__lineno__
    lines = []
    while __next_argv__
      read = @__current_file__.readlines(*args, **options)
      __close_current__ unless __generic_input__
      @__next_p__ = 1
      lines.concat(read)
      @__lineno__ = lineno + lines.size
      __global_store__(:$., @__lineno__) if equal?(ARGF)
    end
    @__init_p__ = 0
    lines
  end
  alias to_a readlines

  # `length` bytes across as many files as it takes, or everything that is
  # left in all of them; nil once they are all read.
  def read(length = nil, buffer = nil)
    wanted = Integer.__c_long__(length) unless length.nil?
    # Emptied as `rb_str_resize` empties it, keeping its encoding.
    (buffer = IO.__str__(buffer)).clear unless buffer.nil?
    out = buffer
    loop do
      return out unless __next_argv__
      read = @__current_file__.read(length)
      if out.nil?
        out = read
      elsif !read.nil?
        out << read
      end
      if read.nil? || length.nil?
        if @__next_p__ != -1
          __close_current__
          @__next_p__ = 1
          next
        end
      elsif out.bytesize < wanted
        length = wanted - out.bytesize
        next
      end
      return out
    end
  end

  # A length and an optional buffer, taken as CRuby's `rb_scan_args` takes
  # them, so the arity is -1 as there.
  def readpartial(*args)
    ARGF.class.__length_and_buffer__(args)
    __getpartial__(:readpartial, args[0], args[1], true, false)
  end

  def read_nonblock(*args, exception: true)
    ARGF.class.__length_and_buffer__(args)
    IO.__exception_flag__(exception)
    __getpartial__(:read_nonblock, args[0], args[1], exception, true)
  end

  def getc
    loop do
      return nil unless __next_argv__
      char = @__current_file__.getc
      return char unless char.nil? && @__next_p__ != -1
      __close_current__
      @__next_p__ = 1
    end
  end

  def getbyte
    loop do
      return nil unless __next_argv__
      byte = @__current_file__.getbyte
      return byte unless byte.nil? && @__next_p__ != -1
      __close_current__
      @__next_p__ = 1
    end
  end

  # As `getc`, but with no stream at all an `EOFError`. Standard input at its
  # end still answers nil, as CRuby's `argf_readchar` does. Measured on 4.0.9.
  def readchar
    loop do
      raise EOFError, "end of file reached" unless __next_argv__
      char = @__current_file__.getc
      return char unless char.nil? && @__next_p__ != -1
      __close_current__
      @__next_p__ = 1
    end
  end

  def readbyte
    return nil unless __next_argv__
    return @__current_file__.readbyte if __generic_input__
    byte = getbyte
    raise EOFError, "end of file reached" if byte.nil?
    byte
  end

  def each_line(*args, **options, &block)
    return to_enum(:each_line, *args, **options) unless block
    while __next_argv__
      __block_call__(:each_line, args, options, true, &block)
    end
    self
  end
  alias each each_line

  def each_byte(&block)
    return to_enum(:each_byte) unless block
    __block_call__(:each_byte, [], {}, false, &block) while __next_argv__
    self
  end

  def each_char(&block)
    return to_enum(:each_char) unless block
    __block_call__(:each_char, [], {}, false, &block) while __next_argv__
    self
  end

  def each_codepoint(&block)
    return to_enum(:each_codepoint) unless block
    __block_call__(:each_codepoint, [], {}, false, &block) while __next_argv__
    self
  end

  def tell
    raise ArgumentError, "no stream to tell" unless __next_argv__
    @__current_file__.tell
  end
  alias pos tell

  def seek(*args)
    raise ArgumentError, "no stream to seek" unless __next_argv__
    @__current_file__.seek(*args)
  end

  def pos=(offset)
    raise ArgumentError, "no stream to set position" unless __next_argv__
    @__current_file__.pos = offset
  end

  # The global `ARGF`'s line number goes back by the file's in `IO#rewind`
  # (`__io_rewound__`); another's goes back here.
  def rewind
    raise ArgumentError, "no stream to rewind" unless __next_argv__
    return @__current_file__.rewind if __generic_input__
    old_lineno = @__current_file__.lineno
    rewound = @__current_file__.rewind
    @__lineno__ -= old_lineno unless equal?(ARGF)
    rewound
  end

  def eof?
    __next_argv__
    if @__current_file__
      return true if @__init_p__ == 0
      __next_argv__
      return @__current_file__.eof? if __generic_input__
      return true if @__current_file__.eof?
    end
    false
  end
  alias eof eof?

  # Every file opened from now on is read in binary mode too.
  def binmode
    @__binmode__ = true
    __next_argv__
    return @__current_file__.binmode if __generic_input__
    @__current_file__.binmode
    self
  end

  def binmode? = @__binmode__

  def skip
    if @__init_p__ != 0 && @__next_p__ == 0
      __close_current__
      @__next_p__ = 1
    end
    self
  end

  # Standard input is never closed. The line number starts again at 0, but
  # `$.` keeps the last line's.
  def close
    __next_argv__
    __close_current__
    @__next_p__ = 1 if @__next_p__ != -1
    @__lineno__ = 0
    self
  end

  def closed?
    __next_argv__
    @__current_file__.closed?
  end

  def external_encoding
    return Encoding.default_external unless @__current_file__
    @__current_file__.external_encoding
  end

  def internal_encoding
    return Encoding.default_external unless @__current_file__
    @__current_file__.internal_encoding
  end

  # The encodings of the file being read, and of every file opened after it.
  def set_encoding(*encodings, **options)
    raise ArgumentError, "no stream to set encoding" unless __next_argv__
    current = @__current_file__
    current.set_encoding(*encodings, **options)
    # A `$stdin` that is no IO has no encodings to keep, as CRuby finds.
    raise IOError, "uninitialized stream" unless IO === current
    @__encs__ = [current.external_encoding, current.internal_encoding, options]
    self
  end

  # CRuby's `ARGF_GENERIC_INPUT_P`: reading `$stdin` when it is no IO, as a
  # StringIO, which is then sent the method itself.
  def __generic_input__
    current = @__current_file__
    current.equal?($stdin) && !(IO === current)
  end

  # CRuby's `argf_next_argv`: whether there is a stream to read, opening the
  # next file named in `argv` when the last one is done.
  def __next_argv__
    if @__init_p__ == 0
      @__next_p__ = !@__argv__.nil? && !@__argv__.empty? ? 1 : -1
      @__init_p__ = 1
    elsif @__argv__.nil?
      @__next_p__ = -1
    elsif @__next_p__ == -1 && !@__argv__.empty?
      @__next_p__ = 1
    end

    if @__next_p__ == 1
      __close_current__ if @__init_p__ == 1
      if @__argv__.empty?
        @__next_p__ = 1
        return false
      end
      filename = File.__path__(@__argv__.shift)
      @__filename__ = filename
      if filename == "-"
        @__current_file__ = $stdin
      else
        @__current_file__ = File.new(filename, "r")
      end
      current = @__current_file__
      # CRuby takes the IO's state here, which a `$stdin` that is no IO
      # does not have, and the file stays the next one to read. Measured
      # on 4.0.9 with a StringIO.
      raise IOError, "uninitialized stream" unless IO === current
      current.binmode if @__binmode__
      unless @__encs__.nil?
        external, internal, options = @__encs__
        current.set_encoding(external, internal, **options)
      end
      @__next_p__ = 0
    elsif @__next_p__ == -1
      @__current_file__ = $stdin
      @__filename__ = +"-"
    end
    @__init_p__ = 1 if @__init_p__ == -1
    true
  end

  # CRuby's `argf_close`: done with the file being read, though never with
  # standard input.
  def __close_current__
    file = @__current_file__
    return if file.equal?($stdin)
    ARGF.class.__close__(file)
    @__init_p__ = -1
  end

  # CRuby's `argf_getline`: the next line of the stream, going on to the next
  # file at the end of each.
  def __getline__(args, options)
    lineno = @__lineno__
    loop do
      return nil unless __next_argv__
      line = @__current_file__.gets(*args, **options)
      if line.nil? && @__next_p__ != -1 && !__generic_input__
        __close_current__
        @__next_p__ = 1
        next
      end
      unless line.nil?
        @__lineno__ = lineno + 1
        __global_store__(:$., @__lineno__) if equal?(ARGF)
      end
      return line
    end
  end

  # CRuby's `argf_getpartial`: what is there in the file being read. At the
  # end of one that is not the last, an empty String; at the end of the last,
  # `EOFError`, or nil when `exception: false` says so.
  def __getpartial__(name, length, buffer, exception, nonblock)
    buffer = IO.__str__(buffer) unless buffer.nil?
    unless __next_argv__
      buffer&.clear
      raise EOFError, "end of file reached"
    end
    current = @__current_file__
    read = begin
      if __generic_input__
        current.public_send(name, length, buffer)
      elsif nonblock
        current.read_nonblock(length, buffer, exception: exception)
      else
        current.readpartial(length, buffer)
      end
    rescue EOFError
      nil
    end
    return read unless read.nil?
    if @__next_p__ != -1
      __close_current__
      @__next_p__ = 1
      return buffer.nil? ? "".b : buffer unless @__argv__.empty?
    end
    raise EOFError, "end of file reached" if exception
    nil
  end

  # CRuby's `argf_block_call`: the file being read's own `each_*`, until it
  # ends or the block closes it or moves `ARGF` on to another, which ends
  # this file's part. A line counts in this `ARGF`'s line number as it is
  # read; the global one counts it in `IO#gets`.
  def __block_call__(name, args, options, lines)
    current = @__current_file__
    moved = false
    current.__send__(name, *args, **options) do |*values|
      @__lineno__ += 1 if lines && !equal?(ARGF)
      yield(*values)
      if @__init_p__ == -1 || !current.equal?(@__current_file__)
        moved = true
        break
      end
    end
    @__next_p__ = 1 unless moved
  end

  # `IO#gets` and its kin read `line` as `io`'s line `lineno`, the number
  # `$.` takes: this `ARGF`'s own when `io` is the file it is reading.
  def __io_line__(io, lineno)
    return lineno unless io.equal?(@__current_file__)
    @__lineno__ += 1
  end

  # `$. = value`, which is this `ARGF`'s line number too (CRuby's
  # `argf_lineno_setter`); `core/warning.rb` converts it.
  def __lineno_set__(value)
    @__lineno__ = value
  end

  # `IO#rewind` took the file it is reading back by `lineno` lines.
  def __io_rewound__(io, lineno)
    @__lineno__ -= lineno if io.equal?(@__current_file__)
  end

  private :__generic_input__, :__next_argv__, :__close_current__, :__getline__, :__getpartial__,
          :__block_call__, :__lineno_set__

  # `NUM2INT`, which `ARGF.lineno=` and `$.` take their value by: a Float
  # truncated, anything else by `to_int`, and either held to a C int.
  def self.__c_int__(value)
    IO.__c_int__(Float === value ? Array.__long__(value) : value)
  end

  def self.__length_and_buffer__(args)
    return if args.size == 1 || args.size == 2
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
  end

  # CRuby's `io_close`: `close` unless `closed?` says it already is, and an
  # IOError for a stream closed meanwhile is no error.
  def self.__close__(io)
    closed = Kernel.__check_funcall__(io, :closed?) { nil }
    return if closed
    begin
      Kernel.__check_funcall__(io, :close) { nil }
    rescue IOError => error
      raise unless error.message == "closed stream"
    end
  end
end.new
ARGF.class.set_temporary_name("ARGF.class")
ARGF.instance_variable_set(:@__argv__, ARGV)
ARGF.instance_variable_set(:@__filename__, +"-")

# `$<` is `ARGF`, and `$FILENAME` reads `ARGF.filename`; both are read-only.
# `$.` is in `core/warning.rb` with the other globals assigned by a rule.
__global_store__(:$<, ARGF)
__hook_global__(true, :$FILENAME)
__global_store__(:$FILENAME, nil)
__freeze_global__(:$<, :$FILENAME)
