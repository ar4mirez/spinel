# IO::Buffer (#797).
#
# CRuby's buffer is four facts — where its memory starts, how long it is, its
# flags, and the buffer or String a slice was cut from — and so is this one,
# in hidden instance variables:
#
#   @__base__    the memory: nil (a NULL buffer), a String, or a Mapping
#   @__offset__  where in that memory the buffer's bytes start
#   @__size__    how many bytes
#   @__flags__   the Integer flags below, as `io_buffer.c` keeps them
#   @__source__  for a slice, the buffer it was cut from (never a slice), or
#                the String a `for` or `string` buffer stands on; else nil
#
# The memory is one of three things. An INTERNAL buffer's is a hidden BINARY
# String the collector owns. An EXTERNAL one made by `for` or `string` is the
# program's own String, written in place with `__splice__`, which keeps its
# length; inside the block it is locked (`__locktmp__`), so every mutator of it
# raises "can't modify string; temporarily locked", as `rb_str_locktmp` makes
# it. A MAPPED one's is a Mapping: memory `mmap(2)` made, which the heap's
# table in `spinel-vm/src/iobuf.rs` owns and bounds-checks every read and write
# against. A Mapping unmaps when `free`d, or when the collector finds nothing
# holds it, by the finalizer its owner object carries — the shape
# `Fiddle::Pointer` uses — so `transfer` handing it to a new buffer never
# leaves the old one able to unmap it.
#
# A slice shares its source's memory. It is valid while its source still has
# that memory and the slice's range lies inside the source's, as
# `io_buffer_validate_slice` compares base pointers; `null?` is whether it has
# memory at all, which a slice of a freed buffer still does.
#
# Every rule and message was measured on ruby 3.4.10, and checked against the
# examples ruby/spec guards for 4.0 where the two differ.
class IO
  class Buffer
    include Comparable

    class LockedError < RuntimeError; end
    class AllocationError < RuntimeError; end
    class AccessError < RuntimeError; end
    class InvalidatedError < RuntimeError; end
    class MaskError < ArgumentError; end

    EXTERNAL = 1
    INTERNAL = 2
    MAPPED = 4
    SHARED = 8
    LOCKED = 32
    PRIVATE = 64
    READONLY = 128

    # A mapping of a file: shown by `to_s`, never a constant.
    FILE_FLAG = 16
    private_constant :FILE_FLAG
    FLAGS_MASK = EXTERNAL | INTERNAL | MAPPED | SHARED | FILE_FLAG | LOCKED | PRIVATE | READONLY
    private_constant :FLAGS_MASK

    LITTLE_ENDIAN = 4
    BIG_ENDIAN = 8
    HOST_ENDIAN = [1].pack("S") == "\x01\x00".b ? LITTLE_ENDIAN : BIG_ENDIAN
    NETWORK_ENDIAN = BIG_ENDIAN

    PAGE_SIZE = __iobuf_page_size__

    # `io_buffer_default_size`: 64KiB rounded up to whole pages, or what
    # `RUBY_IO_BUFFER_DEFAULT_SIZE` says.
    DEFAULT_SIZE = begin
      custom = ENV["RUBY_IO_BUFFER_DEFAULT_SIZE"].to_i
      if custom > 0 && custom < 2**31
        custom
      elsif PAGE_SIZE > 65536
        PAGE_SIZE
      else
        (65536 + PAGE_SIZE - 1) / PAGE_SIZE * PAGE_SIZE
      end
    end

    # Whether a mapping can be resized where it is, as CRuby's `resize` does
    # with `mremap(2)` where there is one: Linux. Elsewhere a mapped buffer
    # resized is a fresh one, INTERNAL or MAPPED as its new size says.
    REMAP = __ruby_constants__.then { |pairs| pairs[pairs.index("RUBY_PLATFORM") + 1] }.include?("linux")
    private_constant :REMAP

    # What `inspect` hexdumps of a buffer at most, and how wide.
    INSPECT_HEXDUMP_MAXIMUM = 256
    private_constant :INSPECT_HEXDUMP_MAXIMUM

    # The data types: bytes wide, the `pack` directive, and how a value is
    # converted to be written, as the C conversion `io_buffer.c` uses for
    # each converts it.
    TYPES = {
      U8: [1, "C", :uint], S8: [1, "c", :int],
      u16: [2, "S<", :uint], U16: [2, "S>", :uint], s16: [2, "s<", :int], S16: [2, "s>", :int],
      u32: [4, "L<", :uint], U32: [4, "L>", :uint], s32: [4, "l<", :int], S32: [4, "l>", :int],
      u64: [8, "Q<", :ull], U64: [8, "Q>", :ull], s64: [8, "q<", :ll], S64: [8, "q>", :ll],
      f32: [4, "e", :double], F32: [4, "g", :double], f64: [8, "E", :double], F64: [8, "G", :double],
    }.freeze
    private_constant :TYPES

    # Memory `mmap(2)` made, named by its address in the heap's table. The
    # finalizer is its own, over `@__state__` rather than over the object, so
    # it can run once nothing holds the Mapping; `release` empties the state,
    # which leaves the finalizer nothing to do. `@__shown__` is where it was
    # last, which a slice of it still prints once it is gone, as CRuby's
    # slice keeps its pointer.
    class Mapping
      def initialize(address, size)
        @__state__ = [address, size]
        @__shown__ = address
        ObjectSpace.define_finalizer(self, Mapping.__releaser__(@__state__))
      end

      def shown = @__shown__

      def self.__releaser__(state)
        proc do
          address = state[0]
          state[0] = nil
          __iobuf_unmap__(address) unless address.nil?
        end
      end

      def address = @__state__[0]
      def size = @__state__[1]

      def release
        address = @__state__[0]
        return if address.nil?
        @__state__[0] = nil
        __iobuf_unmap__(address)
        nil
      end

      # `mremap(2)`: the bytes kept as far as they fit, and the mapping
      # perhaps somewhere else now.
      def remap(size)
        address = Mapping.__check__(__iobuf_remap__(@__state__[0], size), "rb_io_buffer_resize:mremap")
        @__state__[0] = address
        @__state__[1] = size
        @__shown__ = address
        self
      end

      # Anonymous memory, zero-filled; `MAP_SHARED` when `shared`.
      def self.anonymous(size, shared)
        new(__check__(__iobuf_map__(-1, size, 0, shared, true), "io_buffer_map_memory:mmap"), size)
      end

      def self.file(fd, size, offset, shared, writable)
        new(__check__(__iobuf_map__(fd, size, offset, shared, writable), "io_buffer_map_file:mmap"), size)
      end

      # The address a primitive answered, or the `SystemCallError` of its
      # errno, worded as `rb_sys_fail(where)` words it.
      def self.__check__(result, where)
        raise SystemCallError.new(where, result) if Integer === result
        result[0]
      end
    end
    private_constant :Mapping

    # --- making one ----------------------------------------------------------

    # `io_buffer_experimental`: once per process, when experimental warnings
    # are on.
    def self.__experimental__
      return if @__warned__
      @__warned__ = true
      __warning__("IO::Buffer is experimental and both the Ruby and C interface may change in the future!", false, :experimental)
    end

    def initialize(*args)
      IO::Buffer.__arity__(args, 0, 2)
      IO::Buffer.__experimental__
      size = args.empty? ? DEFAULT_SIZE : IO::Buffer.__size__(args[0])
      flags = args.size == 2 ? IO::Buffer.__flags__(args[1]) : IO::Buffer.__flags_for_size__(size)
      __clear__
      __allocate__(size, flags)
      self
    end

    # `io_buffer_initialize` with no memory given: nothing at all for no
    # bytes, whatever the flags; INTERNAL before MAPPED; neither is an
    # `AllocationError`.
    def __allocate__(size, flags)
      return if size == 0
      if flags & INTERNAL != 0
        @__base__ = IO::Buffer.__zeroes__(size)
      elsif flags & MAPPED != 0
        @__base__ = Mapping.anonymous(size, flags & SHARED != 0)
      else
        raise AllocationError, "Could not allocate buffer!"
      end
      @__offset__ = 0
      @__size__ = size
      @__flags__ = flags
    end

    def __clear__
      @__base__ = nil
      @__offset__ = 0
      @__size__ = 0
      @__flags__ = 0
      @__source__ = nil
    end

    # A zero-filled BINARY String, or the `AllocationError` of memory that
    # cannot be had.
    def self.__zeroes__(size)
      "\0".b * size
    rescue ArgumentError, NoMemoryError
      raise AllocationError, "Could not allocate buffer!"
    end

    def self.__flags_for_size__(size)
      size >= PAGE_SIZE ? MAPPED : INTERNAL
    end

    # `io_buffer_extract_size` and friends: an Integer, not negative.
    def self.__size__(value, what = "Size")
      raise TypeError, "not an Integer" unless Integer === value
      raise ArgumentError, "#{what} can't be negative!" if value < 0
      if value > 18446744073709551615
        raise RangeError, "bignum too big to convert into 'unsigned long'"
      end
      value
    end

    def self.__offset__(value) = __size__(value, "Offset")
    def self.__length__(value) = __size__(value, "Length")
    def self.__count__(value) = __size__(value, "Count")

    # `rb_check_arity` over what follows `before` fixed arguments.
    def self.__arity__(args, min, max, before = 0)
      return if args.size >= min && args.size <= max
      given = args.size + before
      raise ArgumentError, "wrong number of arguments (given #{given}, expected #{min + before}..#{max + before})"
    end

    # `io_buffer_extract_flags`: unknown flags are dropped, as CRuby drops
    # them for any added later.
    def self.__flags__(value)
      raise TypeError, "not an Integer" unless Integer === value
      raise ArgumentError, "Flags can't be negative!" if value < 0
      IO.__c_uint__(value) & FLAGS_MASK
    end

    # A buffer over the String `string`, which is its source: what `for` and
    # `string` make, as `io_buffer_for_make_instance` makes it.
    def self.__for_instance__(string, flags)
      flags |= EXTERNAL
      flags |= READONLY if string.frozen?
      string.__modify__ if flags & READONLY == 0
      buffer = allocate
      buffer.__external__(string, flags)
      buffer
    end

    def __external__(string, flags)
      @__base__ = string
      @__offset__ = 0
      @__size__ = string.bytesize
      @__flags__ = flags
      @__source__ = string
    end

    def self.for(string)
      __experimental__
      string = IO.__str__(string)
      unless block_given?
        # `rb_str_tmp_frozen_acquire`: the String itself when it is frozen,
        # else a frozen copy, so the buffer never sees a later change.
        string = String.new(string).freeze unless string.frozen?
        return __for_instance__(string, READONLY)
      end
      instance = nil
      begin
        instance = __for_instance__(string, 0)
        string.__locktmp__(true)
        yield instance
      ensure
        instance&.__free_locked__
        string.__locktmp__(false)
      end
    end

    def self.string(length)
      length = __long__(length)
      raise ArgumentError, "negative string size (or size too big)" if length < 0
      string = "\0".b * length
      instance = nil
      begin
        instance = __for_instance__(string, 0)
        string.__locktmp__(true)
        raise LocalJumpError, "no block given" unless block_given?
        yield instance
      ensure
        instance&.__free_locked__
        string.__locktmp__(false)
      end
      string
    end

    # `NUM2LONG`.
    def self.__long__(value)
      value = Integer.__index__(value)
      if value > 9223372036854775807 || value < -9223372036854775808
        raise RangeError, "bignum too big to convert into 'long'"
      end
      value
    end

    def self.map(io, *args)
      __arity__(args, 0, 3, 1)
      size = args[0]
      flags = args.size >= 3 ? args[2] : 0
      __experimental__
      file_size = __file_size__(io)
      raise ArgumentError, "Invalid negative or zero file size!" if file_size <= 0
      unless size.nil?
        size = __size__(size)
        raise ArgumentError, "Size can't be zero!" if size == 0
        raise ArgumentError, "Size can't be larger than file size!" if size > file_size
      end
      offset = 0
      if args.size >= 2
        size_given = !size.nil?
        offset = Integer.__index__(args[1])
        raise ArgumentError, "Offset can't be negative!" if offset < 0
        raise ArgumentError, "Offset too large!" if offset >= file_size
        if !size_given
          size = file_size - offset
        elsif file_size - offset < size
          raise ArgumentError, "Offset too large!"
        end
      end
      size = file_size if size.nil?
      flags = __flags__(flags)
      # Always an `IO::Buffer`, as `rb_io_buffer_map` makes `rb_cIOBuffer`.
      buffer = IO::Buffer.allocate
      buffer.__map_file__(__descriptor__(io), size, offset, flags)
      buffer
    end

    # `rb_file_size`: an IO's by `fstat(2)`, once what it has buffered to
    # write is written; anything else's `size`.
    def self.__file_size__(io)
      if IO === io
        io.flush unless io.closed?
        io.stat.size
      else
        Integer.__index__(io.size)
      end
    end

    # `rb_io_descriptor`: an IO's descriptor, or what `fileno` answers.
    def self.__descriptor__(io)
      return io.fileno if IO === io
      unless io.respond_to?(:fileno)
        raise TypeError, "expected IO or #fileno, #{io.class} given"
      end
      IO.__c_int__(io.fileno)
    end

    # `io_buffer_map_file`.
    def __map_file__(fd, size, offset, flags)
      __clear__
      readonly = flags & READONLY != 0
      shared = flags & PRIVATE == 0
      made = readonly ? READONLY : 0
      made |= shared ? EXTERNAL | SHARED : PRIVATE
      @__base__ = Mapping.file(fd, size, offset, shared, !readonly)
      @__size__ = size
      @__flags__ = made | MAPPED | FILE_FLAG
    end

    # A buffer the scheduler hooks hand over (#797), as
    # `rb_io_buffer_new(base, size, RB_IO_BUFFER_LOCKED)` makes one over the
    # memory a read fills or a write sends: neither INTERNAL nor EXTERNAL,
    # and locked. `string` is what it reads and writes.
    def self.__locked_over__(string, readonly)
      buffer = allocate
      buffer.__over__(string, LOCKED | (readonly ? READONLY : 0))
      buffer
    end

    def __over__(string, flags)
      @__base__ = string
      @__offset__ = 0
      @__size__ = string.bytesize
      @__flags__ = flags
      @__source__ = nil
    end

    # --- the scheduler's IO hooks ---------------------------------------------
    # An IO read or write in a non-blocking fiber is the scheduler's when it
    # has the hook, made on a locked buffer over the bytes, as
    # `rb_fiber_scheduler_io_read_memory` and `_write_memory` make it, and
    # freed when the hook returns. The hook answers what it moved, or a
    # negative errno (`rb_fiber_scheduler_io_result_apply`). These answer as
    # the descriptor primitives do — the bytes read, `[written]`, or a
    # positive errno — so the IO goes on as though it had made the call; nil
    # means the scheduler has no such hook, and the IO makes the call itself.

    def self.__scheduler_read__(scheduler, io, size, from = nil)
      hook = from.nil? ? :io_read : :io_pread
      return nil unless scheduler.respond_to?(hook, true)
      memory = "\0".b * size
      buffer = __locked_over__(memory, false)
      begin
        result = if from.nil?
          scheduler.__send__(hook, io, buffer, 0, 0)
        else
          scheduler.__send__(hook, io, buffer, from, 0, 0)
        end
      ensure
        buffer.__free_locked__
      end
      moved = Integer.__index__(result)
      return -moved if moved < 0
      memory.byteslice(0, moved)
    end

    def self.__scheduler_write__(scheduler, io, bytes, from = nil)
      hook = from.nil? ? :io_write : :io_pwrite
      return nil unless scheduler.respond_to?(hook, true)
      buffer = __locked_over__(bytes, true)
      begin
        result = if from.nil?
          scheduler.__send__(hook, io, buffer, 0, 0)
        else
          scheduler.__send__(hook, io, buffer, from, 0, 0)
        end
      ensure
        buffer.__free_locked__
      end
      moved = Integer.__index__(result)
      moved < 0 ? -moved : [moved]
    end

    # `rb_io_buffer_initialize_copy`: a fresh buffer of the same size,
    # INTERNAL or MAPPED as `new` would make it, holding the same bytes.
    def initialize_copy(source)
      IO::Buffer.__check_buffer__(source)
      bytes = source.__bytes_for_reading__
      __clear__
      __allocate__(bytes.bytesize, IO::Buffer.__flags_for_size__(bytes.bytesize))
      __write_bytes__(0, bytes) unless bytes.empty?
      self
    end

    # --- what it is ------------------------------------------------------------

    def size = @__size__ || 0
    def null? = @__base__.nil?
    def empty? = size == 0
    def external? = __flag__?(EXTERNAL)
    def internal? = __flag__?(INTERNAL)
    def mapped? = __flag__?(MAPPED)
    def shared? = __flag__?(SHARED)
    def locked? = __flag__?(LOCKED)
    def private? = __flag__?(PRIVATE)
    def readonly? = __flag__?(READONLY)

    def __flag__?(flag) = (@__flags__ || 0) & flag != 0
    def __base__ = @__base__
    def __offset__ = @__offset__ || 0
    def __source__ = @__source__

    # `io_buffer_validate`: a buffer that is no slice is valid; a slice is
    # while its source has memory, the same memory, around the slice's range.
    def valid?
      source = @__source__
      return true if source.nil?
      if String === source
        return false unless @__base__.equal?(source)
        return __offset__ + size <= source.bytesize
      end
      base = source.__base__
      return false if base.nil? || !base.equal?(@__base__)
      start = source.__offset__
      __offset__ >= start && __offset__ + size <= start + source.size
    end

    # Where the buffer's bytes are, as `%p` prints a pointer: a mapping's
    # address, or its String's.
    def __base_address__
      base = @__base__
      return 0 if base.nil?
      at = Mapping === base ? base.shown : base.__address__.to_i(16)
      at + __offset__
    end

    def to_s
      text = +"#<#{self.class} 0x#{__base_address__.to_s(16).rjust(16, "0")}+#{size}"
      text << " NULL" if null?
      text << " EXTERNAL" if external?
      text << " INTERNAL" if internal?
      text << " MAPPED" if mapped?
      text << " FILE" if __flag__?(FILE_FLAG)
      text << " SHARED" if shared?
      text << " LOCKED" if locked?
      text << " PRIVATE" if private?
      text << " READONLY" if readonly?
      text << " SLICE" unless @__source__.nil?
      text << " INVALID" unless valid?
      text << ">"
    end

    def inspect
      text = to_s
      return text unless valid?
      shown = size > INSPECT_HEXDUMP_MAXIMUM ? INSPECT_HEXDUMP_MAXIMUM : size
      text << IO::Buffer.__hexdump__(__read_bytes__(0, shown), 0, 16, false) if shown > 0
      text << "\n(and #{size - shown} more bytes not printed)" if shown < size
      text
    end

    # `io_buffer_hexdump`: a line per `width` bytes, the offset, the bytes in
    # hex and the printable ones as text.
    def self.__hexdump__(bytes, offset, width, first)
      out = +""
      at = 0
      while at < bytes.bytesize
        out << (first ? "" : "\n") << "0x" << (offset + at).to_s(16).rjust(8, "0") << " "
        first = false
        text = +""
        i = 0
        while i < width
          if at + i < bytes.bytesize
            byte = bytes.getbyte(at + i)
            text << (byte >= 32 && byte < 127 ? byte.chr : ".")
            out << " " << byte.to_s(16).rjust(2, "0")
          else
            out << "   "
          end
          i += 1
        end
        out << " " << text
        at += width
      end
      out
    end

    def hexdump(*args)
      IO::Buffer.__arity__(args, 0, 3)
      offset, length = __offset_length__(args[0], args[1])
      width = 16
      if args.size >= 3
        width = IO::Buffer.__size__(args[2], "Width")
        raise ArgumentError, "Width must be at least 1!" if width < 1
      end
      __validate_range__(offset, length)
      return nil unless valid? && !null?
      IO::Buffer.__hexdump__(__read_bytes__(offset, length), offset, width, true)
    end

    # --- reading and writing its bytes -----------------------------------------

    # The bytes from `offset`, `length` of them, wherever the memory is. The
    # caller has checked the range is the buffer's.
    def __read_bytes__(offset, length)
      base = @__base__
      return "".b if base.nil? || length == 0
      at = __offset__ + offset
      if Mapping === base
        address = base.address
        bytes = address.nil? ? nil : __iobuf_read__(address, at, length)
        raise InvalidatedError, "Buffer has been invalidated!" if bytes.nil?
        bytes
      else
        bytes = base.byteslice(at, length)
        raise InvalidatedError, "Buffer has been invalidated!" if bytes.nil? || bytes.bytesize != length
        bytes.force_encoding(Encoding::BINARY)
      end
    end

    def __write_bytes__(offset, bytes)
      base = @__base__
      return if base.nil? || bytes.empty?
      at = __offset__ + offset
      if Mapping === base
        address = base.address
        raise InvalidatedError, "Buffer is invalid!" if address.nil? || !__iobuf_write__(address, at, bytes)
      else
        raise InvalidatedError, "Buffer is invalid!" if at + bytes.bytesize > base.bytesize
        base.__splice__(at, bytes.bytesize, bytes)
      end
    end

    def __fill_bytes__(offset, length, byte)
      return if length == 0 || @__base__.nil?
      base = @__base__
      if Mapping === base
        address = base.address
        raise InvalidatedError, "Buffer is invalid!" if address.nil? || !__iobuf_fill__(address, __offset__ + offset, length, byte)
      else
        __write_bytes__(offset, byte.chr.b * length)
      end
    end

    # `io_buffer_get_bytes_for_reading`: every byte, once the buffer is found
    # valid.
    def __bytes_for_reading__
      __readable__
      __read_bytes__(0, size)
    end

    def __readable__
      raise InvalidatedError, "Buffer has been invalidated!" unless valid?
    end

    # `io_buffer_get_bytes_for_writing`.
    def __writable__
      if readonly? || (!@__source__.nil? && @__source__.frozen?)
        raise AccessError, "Buffer is not writable!"
      end
      raise InvalidatedError, "Buffer is invalid!" unless valid?
    end

    def __validate_range__(offset, length)
      if offset + length > size
        raise ArgumentError, "Specified offset+length is bigger than the buffer size!"
      end
    end

    # `io_buffer_extract_offset_length`: nil is the default for either.
    def __offset_length__(offset, length)
      offset = offset.nil? ? 0 : IO::Buffer.__offset__(offset)
      if length.nil?
        if offset > size
          raise ArgumentError, "The given offset is bigger than the buffer size!"
        end
        length = size - offset
      else
        length = IO::Buffer.__length__(length)
      end
      [offset, length]
    end

    def get_string(*args)
      IO::Buffer.__arity__(args, 0, 3)
      offset, length = __offset_length__(args[0], args[1])
      __readable__
      encoding = args.size >= 3 ? Encoding.find(args[2]) : Encoding::BINARY
      __validate_range__(offset, length)
      __read_bytes__(offset, length).force_encoding(encoding)
    end

    def set_string(string, *args)
      string = String === string ? string : IO.__str__(string)
      __copy_from__(string.b, args)
    end

    def copy(source, *args)
      IO::Buffer.__check_buffer__(source)
      __copy_from__(source.__bytes_for_reading__, args)
    end

    # `io_buffer_copy_from`: `(offset = 0, length = nil, source_offset = 0)`
    # into this buffer, from `bytes`. The bytes are read first, so a copy
    # from an overlapping slice is a `memmove`.
    def __copy_from__(bytes, args)
      if args.size > 3
        raise ArgumentError, "wrong number of arguments (given #{args.size + 1}, expected 1..4)"
      end
      offset = args.size >= 1 ? IO::Buffer.__offset__(args[0]) : 0
      source_offset = 0
      if args.size >= 3
        source_offset = IO::Buffer.__offset__(args[2])
        if source_offset > bytes.bytesize
          raise ArgumentError, "The given source offset is bigger than the source itself!"
        end
      end
      length = args.size >= 2 && !args[1].nil? ? IO::Buffer.__length__(args[1]) : bytes.bytesize - source_offset
      if source_offset + length > bytes.bytesize
        raise ArgumentError, "The computed source range exceeds the size of the source buffer!"
      end
      __writable__
      __validate_range__(offset, length)
      __write_bytes__(offset, bytes.byteslice(source_offset, length))
      length
    end

    def clear(value = 0, offset = nil, length = nil)
      value = IO.__c_uint__(value) & 0xff
      offset, length = __offset_length__(offset, length)
      __writable__
      __validate_range__(offset, length)
      __fill_bytes__(offset, length, value)
      self
    end

    def <=>(other)
      IO::Buffer.__check_buffer__(other)
      mine = __bytes_for_reading__
      theirs = other.__bytes_for_reading__
      return -1 if mine.bytesize < theirs.bytesize
      return 1 if mine.bytesize > theirs.bytesize
      # `memcmp`: the difference of the first bytes that differ.
      i = 0
      while i < mine.bytesize
        a = mine.getbyte(i)
        b = theirs.getbyte(i)
        return a - b unless a == b
        i += 1
      end
      0
    end

    # The TypeError `TypedData_Get_Struct` raises for anything not a buffer.
    def self.__check_buffer__(value)
      return value if IO::Buffer === value
      name = value.nil? || true == value || false == value ? value.inspect : value.class
      raise TypeError, "wrong argument type #{name} (expected IO::Buffer)"
    end

    # --- typed values ------------------------------------------------------------

    def self.__type__(type)
      unless Symbol === type
        name = type.nil? || true == type || false == type ? type.inspect : type.class
        raise TypeError, "wrong argument type #{name} (expected Symbol)"
      end
      TYPES[type] || raise(ArgumentError, "Invalid type name!")
    end

    def self.size_of(type)
      return type.sum { |each| __type__(each)[0] } if Array === type
      __type__(type)[0]
    end

    # The value of type `info` at `offset`, of the buffer's `bytes`.
    def __value_at__(info, offset, total)
      if offset + info[0] > total
        raise ArgumentError, "Type extends beyond end of buffer! (offset=#{offset} > size=#{total})"
      end
      __read_bytes__(offset, info[0]).unpack1(info[1])
    end

    def get_value(type, offset)
      offset = IO::Buffer.__offset__(offset)
      __readable__
      __value_at__(IO::Buffer.__type__(type), offset, size)
    end

    def get_values(types, offset)
      offset = IO::Buffer.__offset__(offset)
      __readable__
      raise ArgumentError, "Argument buffer_types should be an array!" unless Array === types
      types.map do |type|
        info = IO::Buffer.__type__(type)
        value = __value_at__(info, offset, size)
        offset += info[0]
        value
      end
    end

    def set_value(type, offset, value)
      offset = IO::Buffer.__offset__(offset)
      __writable__
      __set_value_at__(IO::Buffer.__type__(type), offset, value)
    end

    def set_values(types, offset, values)
      offset = IO::Buffer.__offset__(offset)
      __writable__
      raise ArgumentError, "Argument buffer_types should be an array!" unless Array === types
      raise ArgumentError, "Argument values should be an array!" unless Array === values
      unless types.size == values.size
        raise ArgumentError, "Argument buffer_types and values should have the same length!"
      end
      types.each_with_index do |type, index|
        offset = __set_value_at__(IO::Buffer.__type__(type), offset, values[index])
      end
      offset
    end

    # Write `value` as type `info` at `offset`: the offset after it.
    def __set_value_at__(info, offset, value)
      if offset + info[0] > size
        raise ArgumentError, "Type extends beyond end of buffer! (offset=#{offset} > size=#{size})"
      end
      __write_bytes__(offset, [IO::Buffer.__unwrap__(value, info[2])].pack(info[1]))
      offset + info[0]
    end

    # A value converted as the C conversion each type uses converts it:
    # `NUM2UINT`, `NUM2INT`, `NUM2ULL`, `NUM2LL` or `NUM2DBL`. The pack
    # directive then keeps the bits that fit, as the cast to the C type does.
    def self.__unwrap__(value, how)
      if how == :double
        return value if Float === value
        return value.to_f if Numeric === value
        name = case value
               when nil then "nil"
               when true then "true"
               when false then "false"
               when String then "string"
               end
        raise TypeError, "no implicit conversion to float from #{name}" unless name.nil?
        raise TypeError, "can't convert #{value.class} into Float"
      end
      if Float === value
        if value.nan? || value.infinite? || value >= 18446744073709551616.0 || value < -9223372036854775808.0
          raise RangeError, "float #{value} out of range of integer"
        end
        value = value.to_i
      elsif value.nil?
        raise TypeError, how == :uint ? "no implicit conversion of nil into Integer" : "no implicit conversion from nil to integer"
      else
        value = Integer.__index__(value)
      end
      case how
      when :uint then IO.__c_uint__(value)
      when :int then IO.__c_int__(value)
      when :ll
        if value > 9223372036854775807 || value < -9223372036854775808
          raise RangeError, "bignum too big to convert into 'long long'"
        end
        value
      else
        if value > 18446744073709551615 || value < -9223372036854775808
          raise RangeError, "bignum too big to convert into 'unsigned long long'"
        end
        value
      end
    end

    # `io_buffer_extract_offset_count`, over `(type, offset, count)`: the
    # count is what fits after the offset unless one is given.
    def __offset_count__(info, args)
      offset = args.size >= 2 ? IO::Buffer.__offset__(args[1]) : 0
      count = args.size >= 3 ? args[2] : nil
      if args.size < 3
        if offset > size
          raise ArgumentError, "The given offset is bigger than the buffer size!"
        end
        count = (size - offset) / info[0]
      else
        count = IO::Buffer.__count__(count)
      end
      [offset, count]
    end

    def each(*args)
      return to_enum(:each, *args) unless block_given?
      IO::Buffer.__arity__(args, 0, 3)
      __readable__
      info = IO::Buffer.__type__(args.empty? ? :U8 : args[0])
      offset, count = __offset_count__(info, args)
      __each_value__(info, offset, count) { |at, value| yield at, value }
      self
    end

    def values(*args)
      IO::Buffer.__arity__(args, 0, 3)
      __readable__
      info = IO::Buffer.__type__(args.empty? ? :U8 : args[0])
      offset, count = __offset_count__(info, args)
      result = []
      __each_value__(info, offset, count) { |_at, value| result << value }
      result
    end

    # `each_byte(offset = 0, count = nil)`, as documented.
    def each_byte(*args)
      return to_enum(:each_byte, *args) unless block_given?
      IO::Buffer.__arity__(args, 0, 2)
      __readable__
      offset, count = __offset_count__(TYPES[:U8], [:U8, *args])
      __each_value__(TYPES[:U8], offset, count) { |_at, value| yield value }
      self
    end

    # The `count` values of type `info` from `offset`, with where each is:
    # as many as lie in the buffer, then the range error of the first that
    # does not, as CRuby's loop meets it.
    #
    # ponytail: the bytes are read a chunk of values at a time, so a block
    # that writes into the buffer further on in the same chunk sees the old
    # values there, where CRuby reads each value as it comes to it. The
    # ceiling is that write-ahead, which no caller in the corpus makes; the
    # upgrade is a read per value once one value's read costs less than the
    # interpreter's 50M-instruction budget allows 64Ki of.
    def __each_value__(info, offset, count)
      width = info[0]
      fit = offset > size ? 0 : (size - offset) / width
      wanted = count < fit ? count : fit
      while wanted > 0
        chunk = wanted < 1024 ? wanted : 1024
        bytes = __read_bytes__(offset, chunk * width)
        values = info[1] == "C" ? bytes.bytes : bytes.unpack("#{info[1]}#{chunk}")
        values.each do |value|
          yield offset, value
          offset += width
        end
        wanted -= chunk
      end
      if count > fit
        raise ArgumentError, "Type extends beyond end of buffer! (offset=#{offset} > size=#{size})"
      end
    end

    # --- what changes the buffer itself -----------------------------------------

    def locked
      raise LockedError, "Buffer already locked!" if locked?
      @__flags__ = (@__flags__ || 0) | LOCKED
      begin
        yield self
      ensure
        @__flags__ &= ~LOCKED
      end
    end

    def free
      raise LockedError, "Buffer is locked!" if locked?
      __free_locked__
      self
    end

    # `io_buffer_free`: a mapping this buffer made is unmapped; a slice's,
    # or a String's, memory is only let go of.
    def __free_locked__
      base = @__base__
      return if base.nil?
      base.release if Mapping === base && mapped? && @__source__.nil?
      __clear__
    end

    def transfer
      raise LockedError, "Cannot transfer ownership of locked buffer!" if locked?
      moved = self.class.allocate
      moved.__take__(@__base__, __offset__, size, @__flags__ || 0, @__source__)
      __clear__
      moved
    end

    def __take__(base, offset, size, flags, source)
      @__base__ = base
      @__offset__ = offset
      @__size__ = size
      @__flags__ = flags
      @__source__ = source
    end

    def resize(new_size)
      new_size = IO::Buffer.__size__(new_size)
      raise LockedError, "Cannot resize locked buffer!" if locked?
      if null?
        __clear__
        __allocate__(new_size, IO::Buffer.__flags_for_size__(new_size))
        return self
      end
      raise AccessError, "Cannot resize external buffer!" if external?
      base = @__base__
      old_size = size
      if REMAP && mapped? && @__source__.nil? && !__flag__?(FILE_FLAG)
        if new_size == 0
          __free_locked__
          return self
        end
        base.remap(new_size)
        @__size__ = new_size
        __fill_bytes__(old_size, new_size - old_size, 0) if new_size > old_size
        return self
      end
      if internal? && @__source__.nil?
        if new_size == 0
          __free_locked__
        elsif new_size < old_size
          base.__splice__(new_size, old_size - new_size, "")
          @__size__ = new_size
        else
          base.__splice__(old_size, 0, "\0".b * (new_size - old_size))
          @__size__ = new_size
        end
        return self
      end
      # Anything else — a slice, a private mapping of a file, a mapping where
      # there is no `mremap` — becomes a fresh buffer holding what fits, as
      # `io_buffer_initialize` plus a copy.
      kept = __read_bytes__(0, old_size < new_size ? old_size : new_size)
      __free_locked__
      __allocate__(new_size, IO::Buffer.__flags_for_size__(new_size))
      __write_bytes__(0, kept)
      self
    end

    def slice(offset = nil, length = nil)
      offset, length = __offset_length__(offset, length)
      __validate_range__(offset, length)
      sliced = self.class.allocate
      source = @__source__.nil? ? self : @__source__
      base = @__base__
      sliced.__take__(base, __offset__ + offset, length, (@__flags__ || 0) & READONLY, source)
      sliced
    end

    # --- bitwise -----------------------------------------------------------------

    def self.__check_mask__(mask)
      __check_buffer__(mask)
      raise MaskError, "Zero-length mask given!" if mask.size == 0
      mask
    end

    # `io_buffer_overlaps`: the same memory, and ranges that meet.
    def __overlaps__?(other)
      return false if @__base__.nil? || !@__base__.equal?(other.__base__)
      a = __offset__
      b = other.__offset__
      a < b + other.size && b < a + size
    end

    # Each byte combined with the mask's, the mask repeated.
    def __masked__(mask)
      bytes = __read_bytes__(0, size)
      mask = mask.__read_bytes__(0, mask.size)
      out = "\0".b * bytes.bytesize
      width = mask.bytesize
      i = 0
      while i < bytes.bytesize
        out.setbyte(i, yield(bytes.getbyte(i), mask.getbyte(i % width)))
        i += 1
      end
      out
    end

    def __result__(bytes)
      result = IO::Buffer.allocate
      result.__allocate__(bytes.bytesize, IO::Buffer.__flags_for_size__(bytes.bytesize))
      result.__write_bytes__(0, bytes)
      result
    end

    def &(mask)
      IO::Buffer.__check_mask__(mask)
      __result__(__masked__(mask) { |a, b| a & b })
    end

    def |(mask)
      IO::Buffer.__check_mask__(mask)
      __result__(__masked__(mask) { |a, b| a | b })
    end

    def ^(mask)
      IO::Buffer.__check_mask__(mask)
      __result__(__masked__(mask) { |a, b| a ^ b })
    end

    def ~
      bytes = __read_bytes__(0, size)
      out = "\0".b * bytes.bytesize
      i = 0
      while i < bytes.bytesize
        out.setbyte(i, ~bytes.getbyte(i) & 0xff)
        i += 1
      end
      __result__(out)
    end

    def __in_place__(mask, &op)
      IO::Buffer.__check_mask__(mask)
      raise MaskError, "Mask overlaps source buffer!" if __overlaps__?(mask)
      __writable__
      __write_bytes__(0, __masked__(mask, &op))
      self
    end

    def and!(mask) = __in_place__(mask) { |a, b| a & b }
    def or!(mask) = __in_place__(mask) { |a, b| a | b }
    def xor!(mask) = __in_place__(mask) { |a, b| a ^ b }

    def not!
      __writable__
      bytes = __read_bytes__(0, size)
      i = 0
      while i < bytes.bytesize
        bytes.setbyte(i, ~bytes.getbyte(i) & 0xff)
        i += 1
      end
      __write_bytes__(0, bytes)
      self
    end

    # --- IO ------------------------------------------------------------------------

    # `io_buffer_extract_length_offset`: `(length = nil, offset = 0)`, the
    # length what is left after the offset.
    def __length_offset__(args, from)
      offset = args.size >= from + 2 ? IO::Buffer.__offset__(args[from + 1]) : 0
      length = args.size >= from + 1 && !args[from].nil? ? IO::Buffer.__length__(args[from]) : size - offset
      [length, offset]
    end


    # `io_buffer_blocking_region`: the buffer is locked while the call runs.
    def __blocking__
      return yield if locked?
      @__flags__ |= LOCKED
      begin
        yield
      ensure
        @__flags__ &= ~LOCKED
      end
    end

    # The scheduler's hook named `hook`, as `rb_check_funcall` calls it:
    # `:undef` when there is no current scheduler or it has no such method.
    def self.__hook__(hook, *args)
      scheduler = Fiber.current_scheduler
      return :undef if scheduler.nil? || !scheduler.respond_to?(hook, true)
      scheduler.__send__(hook, *args)
    end

    # What a read or write answers, as `rb_fiber_scheduler_io_result` makes
    # it: the bytes moved, or a negative errno.
    def __io__(io, length, offset, writing, from = nil)
      fd = IO::Buffer.__descriptor__(io)
      room = size - offset
      total = 0
      __blocking__ do
        loop do
          result = if writing
            bytes = __read_bytes__(offset, room)
            from.nil? ? __fd_write__(fd, bytes, true) : __fd_pwrite__(fd, bytes, from)
          else
            from.nil? ? __fd_read__(fd, room, true) : __fd_pread__(fd, room, from)
          end
          if result == IO.__eintr__
            Thread.__check_interrupts__
            next
          end
          break(total = -result) if Integer === result
          moved = writing ? result[0] : result.bytesize
          __write_bytes__(offset, result) unless writing
          break if moved == 0
          total += moved
          break if total >= length
          offset += moved
          room -= moved
          from += moved unless from.nil?
        end
      end
      total
    end

    def read(io, *args)
      IO::Buffer.__arity__(args, 0, 2, 1)
      length, offset = __length_offset__(args, 0)
      result = IO::Buffer.__hook__(:io_read, io, self, length, offset)
      return result unless result == :undef
      __validate_range__(offset, length)
      __writable__
      __io__(io, length, offset, false)
    end

    def write(io, *args)
      IO::Buffer.__arity__(args, 0, 2, 1)
      __readable__
      length, offset = __length_offset__(args, 0)
      __validate_range__(offset, length)
      result = IO::Buffer.__hook__(:io_write, io, self, length, offset)
      return result unless result == :undef
      __io__(io, length, offset, true)
    end

    def pread(io, from, *args)
      IO::Buffer.__arity__(args, 0, 2, 2)
      from = Integer.__index__(from)
      length, offset = __length_offset__(args, 0)
      result = IO::Buffer.__hook__(:io_pread, io, self, from, length, offset)
      return result unless result == :undef
      __validate_range__(offset, length)
      __writable__
      __io__(io, length, offset, false, from)
    end

    def pwrite(io, from, *args)
      IO::Buffer.__arity__(args, 0, 2, 2)
      from = Integer.__index__(from)
      __readable__
      length, offset = __length_offset__(args, 0)
      __validate_range__(offset, length)
      result = IO::Buffer.__hook__(:io_pwrite, io, self, from, length, offset)
      return result unless result == :undef
      __io__(io, length, offset, true, from)
    end
  end
end
