# `fiddle`'s C half (#105): what CRuby's `fiddle.so` defines — `fiddle.c`,
# `handle.c`, `pointer.c`, `function.c`, `closure.c` and `pinned.c` — in Ruby
# over `Spinel::FFI` (`lib/spinel/ffi.rb`). The gem's Ruby half, upstream's
# `lib/` at the version `Fiddle::VERSION` names, is vendored in `stdlib/` and
# requires this file in place of `fiddle.so`. What each method answers and
# raises follows those files, measured against CRuby 4.0.9's `fiddle` 1.1.8.
#
# One difference is deliberate: Spinel never hands C an address inside its own
# heap. Where CRuby points C at a String's bytes, C gets a copy on the C heap.
# For a `Function#call` argument the copy is written back into the String when
# the call returns, so a C function filling a buffer String fills it here too —
# unless the String is frozen, which CRuby lets C write into and Spinel does
# not, since a frozen literal may be shared.
#
# ponytail: `Pointer[string]` copies the String's bytes when it is made, so
# writes through it do not reach the String, nor later changes to the String
# the Pointer; `Pointer#ref` points at a copy of the address, not at the
# Pointer's own field; `Fiddle.dlwrap` answers a token rather than an object's
# address, which only `dlunwrap` reads back; `Fiddle::MemoryView` is not here.
# Upgrade: pinned heap strings and a VM-side reference table, when the GC can
# pin (docs/engine.md, "Native extensions and FFI").

require "spinel/ffi"

module Fiddle
  class Error < StandardError; end
  class DLError < Error; end
  class ClearedReferenceError < Error; end

  module Types
    VOID = 0
    VOIDP = 1
    CHAR = 2
    UCHAR = -2
    SHORT = 3
    USHORT = -3
    INT = 4
    UINT = -4
    LONG = 5
    ULONG = -5
    LONG_LONG = 6
    ULONG_LONG = -6
    INT8_T = 2
    UINT8_T = -2
    INT16_T = 3
    UINT16_T = -3
    INT32_T = 4
    UINT32_T = -4
    INT64_T = 5
    UINT64_T = -5
    FLOAT = 7
    DOUBLE = 8
    VARIADIC = 9
    CONST_STRING = 10
    SIZE_T = -5
    SSIZE_T = 5
    PTRDIFF_T = 5
    INTPTR_T = 5
    UINTPTR_T = -5
    BOOL = 11
  end

  {
    VOIDP: Types::VOIDP, CHAR: Types::CHAR, SHORT: Types::SHORT, INT: Types::INT,
    LONG: Types::LONG, LONG_LONG: Types::LONG_LONG, INT8_T: Types::CHAR,
    INT16_T: Types::SHORT, INT32_T: Types::INT, INT64_T: Types::INT64_T,
    FLOAT: Types::FLOAT, DOUBLE: Types::DOUBLE, SIZE_T: Types::SIZE_T,
    SSIZE_T: Types::SSIZE_T, PTRDIFF_T: Types::PTRDIFF_T,
    INTPTR_T: Types::INTPTR_T, UINTPTR_T: Types::UINTPTR_T, BOOL: Types::BOOL,
  }.each { |name, code| const_set("ALIGN_#{name}", Spinel::FFI.alignof(code)) }

  {
    VOIDP: Types::VOIDP, CHAR: Types::CHAR, UCHAR: Types::UCHAR,
    SHORT: Types::SHORT, USHORT: Types::USHORT, INT: Types::INT,
    UINT: Types::UINT, LONG: Types::LONG, ULONG: Types::ULONG,
    LONG_LONG: Types::LONG_LONG, ULONG_LONG: Types::ULONG_LONG,
    INT8_T: Types::INT8_T, UINT8_T: Types::UINT8_T, INT16_T: Types::INT16_T,
    UINT16_T: Types::UINT16_T, INT32_T: Types::INT32_T,
    UINT32_T: Types::UINT32_T, INT64_T: Types::INT64_T,
    UINT64_T: Types::UINT64_T, FLOAT: Types::FLOAT, DOUBLE: Types::DOUBLE,
    SIZE_T: Types::SIZE_T, SSIZE_T: Types::SSIZE_T,
    PTRDIFF_T: Types::PTRDIFF_T, INTPTR_T: Types::INTPTR_T,
    UINTPTR_T: Types::UINTPTR_T, CONST_STRING: Types::CONST_STRING,
    BOOL: Types::BOOL,
  }.each { |name, code| const_set("SIZEOF_#{name}", Spinel::FFI.sizeof(code)) }

  WINDOWS = false
  BUILD_RUBY_PLATFORM = RUBY_PLATFORM.dup.freeze
  # What `Fiddle.malloc` and `Pointer.malloc` memory is freed with.
  RUBY_FREE = Spinel::FFI::FREE

  # CRuby's special constants, which `dlwrap` answers for them.
  Qtrue = 20
  Qfalse = 0
  Qnil = 4
  Qundef = 36

  @wrapped = ObjectSpace::WeakMap.new
  @wrap_ids = ObjectSpace::WeakKeyMap.new
  @next_wrap = 0

  class << self
    # `rb_fiddle_type_ensure`: a type code for a code or a C type's name.
    def __type__(type) = Spinel::FFI.type(type)

    def malloc(size) = Spinel::FFI.malloc(Spinel::FFI.ulong(size, "unsigned long"))

    def realloc(address, size)
      Spinel::FFI.realloc(Spinel::FFI.ulong(address), Spinel::FFI.ulong(size))
    end

    def free(address)
      Spinel::FFI.free(Spinel::FFI.ulong(address))
      nil
    end

    # An Integer standing for `object`, which `dlunwrap` reads back.
    def dlwrap(object)
      case object
      when true then Qtrue
      when false then Qfalse
      when nil then Qnil
      when Integer
        object.between?(-(2**61), 2**61 - 1) ? (object << 1) | 1 : __wrap__(object)
      else __wrap__(object)
      end
    end

    def dlunwrap(address)
      address = Spinel::FFI.long(address)
      return true if address == Qtrue
      return false if address == Qfalse
      return nil if address == Qnil
      return address >> 1 if address.odd?
      @wrapped[address]
    end

    private

    def __wrap__(object)
      ids = __collectable__(object) ? @wrap_ids : (@static_ids ||= {}.compare_by_identity)
      id = ids[object]
      return id if id
      id = (@next_wrap += 8) + 0x1000
      @wrapped[id] = object
      ids[object] = id
      id
    end

    # Whether `object` can be a weak key: not a Symbol, a Float or another
    # value that lives for ever.
    def __collectable__(object)
      !(Symbol === object || Float === object || Integer === object)
    end
  end

  # A C string's bytes for C: a NUL-terminated copy on the C heap, which the
  # caller frees. `StringValueCStr` raises for a NUL inside.
  def self.__c_string__(value)
    string = String.__coerce__(value)
    raise ArgumentError, "string contains null byte" if string.include?("\0")
    address = Spinel::FFI.malloc(string.bytesize + 1)
    Spinel::FFI.write(address, string.b) unless string.empty?
    address
  end

  # `rb_Integer` on an address, as `fiddle` takes one from a Pointer, a
  # Function, a Closure or an Integer.
  def self.__address__(value)
    integer = value.respond_to?(:to_int) ? value.to_int : Integer(value)
    Spinel::FFI.ulong(integer)
  end

  # `PTR2NUM`: the address, signed.
  def self.__signed__(address)
    address >= 0x8000_0000_0000_0000 ? address - 0x1_0000_0000_0000_0000 : address
  end

  # --- Fiddle::Handle (handle.c) ----------------------------------------------

  class Handle
    RTLD_GLOBAL = Spinel::FFI::RTLD_GLOBAL
    RTLD_LAZY = Spinel::FFI::RTLD_LAZY
    RTLD_NOW = Spinel::FFI::RTLD_NOW

    def self.__predefined__(address)
      handle = allocate
      handle.instance_variable_set(:@__handle__, address)
      handle.instance_variable_set(:@__open__, true)
      handle.instance_variable_set(:@__close__, false)
      handle.freeze
    end

    DEFAULT = __predefined__(Spinel::FFI::RTLD_DEFAULT)
    NEXT = __predefined__(Spinel::FFI::RTLD_NEXT)

    # A handle closes when collected only if `enable_close` was called.
    def self.__finalizer__(state)
      proc { Spinel::FFI.close(state[0]) if state[1] && state[2] rescue nil }
    end

    def self.sym(name) = __lookup__(Spinel::FFI::RTLD_NEXT, name)
    def self.[](name) = sym(name)

    def self.sym_defined?(name)
      address = Spinel::FFI.symbol(Spinel::FFI::RTLD_NEXT, String.__coerce__(name))
      address && Fiddle.__signed__(address)
    end

    def self.__lookup__(handle, name)
      name = String.__coerce__(name)
      address = Spinel::FFI.symbol(handle, name)
      raise DLError, "unknown symbol \"#{name}\"" unless address
      Fiddle.__signed__(address)
    end

    def initialize(lib = nil, flags = RTLD_LAZY | RTLD_GLOBAL)
      path = lib.nil? ? nil : String.__coerce__(lib)
      raise ArgumentError, "string contains null byte" if path&.include?("\0")
      flags = Spinel::FFI.int(flags)
      begin
        address = Spinel::FFI.open(path, flags)
      rescue Spinel::FFI::Error => error
        raise DLError, error.message
      end
      if @__state__
        # Re-initialized: the old library closes now if it would have on
        # collection, and never again.
        __close_on_collect__ if @__open__ && @__close__
        @__state__[1] = false
      end
      @__handle__ = address
      @__open__ = true
      @__close__ = false
      @__state__ = [address, true, false]
      ObjectSpace.define_finalizer(self, Handle.__finalizer__(@__state__))
      if block_given?
        begin
          yield self
        ensure
          close
        end
      end
      nil
    end

    def close
      raise DLError, "dlclose() called too many times" unless @__open__
      @__open__ = false
      @__state__[1] = false if @__state__
      begin
        Spinel::FFI.close(@__handle__)
      rescue Spinel::FFI::Error => error
        raise DLError, error.message
      end
      0
    end

    def enable_close
      @__close__ = true
      @__state__[2] = true if @__state__
      nil
    end

    def disable_close
      @__close__ = false
      @__state__[2] = false if @__state__
      nil
    end

    def close_enabled? = @__close__ ? true : false

    def to_i = Fiddle.__signed__(@__handle__ || 0)

    def to_ptr = Pointer.new(to_i)

    def sym(name)
      raise DLError, "closed handle" unless @__open__
      Handle.__lookup__(@__handle__, name)
    end

    def [](name) = sym(name)

    def sym_defined?(name)
      raise DLError, "closed handle" unless @__open__
      address = Spinel::FFI.symbol(@__handle__, String.__coerce__(name))
      address && Fiddle.__signed__(address)
    end

    # The path the library was loaded from: the link map's `l_name`, which
    # `dlinfo(RTLD_DI_LINKMAP)` gives on Linux. Elsewhere there is no
    # `dlinfo`, and CRuby answers nil.
    def file_name
      dlinfo = Spinel::FFI.symbol(Spinel::FFI::RTLD_DEFAULT, "dlinfo")
      return nil unless dlinfo
      out = Spinel::FFI.malloc(Fiddle::SIZEOF_VOIDP)
      begin
        status, = Spinel::FFI.call(dlinfo, [Types::VOIDP, Types::INT, Types::VOIDP], Types::INT,
          [@__handle__, 2, out])
        link_map = Spinel::FFI.read(out, Fiddle::SIZEOF_VOIDP).unpack1("J")
        if status != 0 || link_map.zero?
          message = Spinel::FFI.symbol(Spinel::FFI::RTLD_DEFAULT, "dlerror")
          text = message && Spinel::FFI.call(message, [], Types::VOIDP, [])[0]
          detail = text && text != 0 ? Spinel::FFI.string(text) : nil
          raise DLError, detail ? "could not get handle file name: #{detail}" : "could not get handle file name"
        end
        name = Spinel::FFI.read(link_map + Fiddle::SIZEOF_VOIDP, Fiddle::SIZEOF_VOIDP).unpack1("J")
        name.zero? ? "".b : Spinel::FFI.string(name)
      ensure
        Spinel::FFI.free(out)
      end
    end

    private

    def __close_on_collect__
      Spinel::FFI.close(@__handle__) rescue nil
    end
  end

  # --- Fiddle::Pointer (pointer.c) --------------------------------------------

  class Pointer
    # Frees what a collected Pointer still owns, with its free function.
    def self.__finalizer__(state)
      proc { Pointer.__release__(state) }
    end

    # `state` is `[address, free function, freed]`.
    def self.__release__(state)
      address, function, freed = state
      return if freed || function.nil? || address.zero?
      state[2] = true
      if function == Spinel::FFI::FREE
        Spinel::FFI.free(address)
      else
        Spinel::FFI.call(function, [Types::VOIDP], Types::VOID, [address])
      end
      nil
    end

    def self.malloc(size, free = nil)
      size = Spinel::FFI.long(size)
      function = __free_function__(free)
      pointer = allocate
      address = Spinel::FFI.malloc(size)
      pointer.__set__(address, size, function)
      pointer.instance_variable_set(:@__wrap__, free) unless free.nil? || Integer === free
      if block_given?
        unless function
          raise ArgumentError, "a free function must be supplied to Fiddle::Pointer.malloc when it is called with a block"
        end
        begin
          yield pointer
        ensure
          pointer.call_free
        end
      else
        pointer
      end
    end

    def self.__free_function__(free)
      free.nil? ? nil : Fiddle.__address__(free)
    end

    def self.to_ptr(value)
      if IO === value
        pointer = new(__stdio_file__(value))
      elsif String === value
        # ponytail: a copy of the String's bytes rather than the bytes
        # themselves; see the top of this file.
        address = Spinel::FFI.malloc(value.bytesize + 1)
        Spinel::FFI.write(address, value.b) unless value.empty?
        pointer = allocate
        pointer.__set__(address, value.bytesize, Spinel::FFI::FREE)
        pointer.instance_variable_set(:@__wrap__, value)
      elsif value.respond_to?(:to_ptr)
        pointer = value.to_ptr
        raise DLError, "to_ptr should return a Fiddle::Pointer object" unless Pointer === pointer
      else
        pointer = new(Integer(value))
      end
      pointer
    end

    def self.[](value) = to_ptr(value)

    # The `FILE *` `rb_io_stdio_file` gives an IO: `fdopen(3)` on its
    # descriptor, made once per IO.
    def self.__stdio_file__(io)
      @stdio_files ||= ObjectSpace::WeakKeyMap.new
      file = @stdio_files[io]
      return file if file
      fdopen = Spinel::FFI.symbol(Spinel::FFI::RTLD_DEFAULT, "fdopen")
      mode = Fiddle.__c_string__(__stdio_mode__(io.fileno))
      begin
        file, = Spinel::FFI.call(fdopen, [Types::INT, Types::VOIDP], Types::VOIDP, [io.fileno, mode])
      ensure
        Spinel::FFI.free(mode)
      end
      @stdio_files[io] = Fiddle.__signed__(file)
    end

    # The `fdopen(3)` mode for descriptor `fd`'s access mode, `F_GETFL`'s.
    def self.__stdio_mode__(fd)
      fcntl = Spinel::FFI.symbol(Spinel::FFI::RTLD_DEFAULT, "fcntl")
      flags, = Spinel::FFI.call(fcntl, [Types::INT, Types::INT], Types::INT, [fd, 3])
      case flags & 3
      when 0 then "r"
      when 1 then "w"
      else "r+"
      end
    end

    def self.read(address, length)
      Spinel::FFI.read(Spinel::FFI.ulong(address), Spinel::FFI.ulong(length))
    end

    def self.write(address, string)
      bytes = String.__coerce__(string)
      Spinel::FFI.write(Spinel::FFI.ulong(address), bytes.b)
      string
    end

    def initialize(address, size = nil, free = nil)
      number = Fiddle.__address__(address)
      size = size.nil? ? 0 : Spinel::FFI.long(size)
      function = Pointer.__free_function__(free)
      return nil if number.zero?
      Pointer.__release__(@__state__) if @__state__
      @__wrap__ = address unless Integer === address
      @__free_wrap__ = free unless free.nil? || Integer === free
      __set__(number, size, function)
      nil
    end

    def __set__(address, size, function)
      @__address__ = address
      @__size__ = size
      @__free__ = function
      @__state__ = [address, function, false]
      ObjectSpace.define_finalizer(self, Pointer.__finalizer__(@__state__)) if function
      self
    end

    def free=(function)
      function = Pointer.__free_function__(function)
      had = @__free__
      @__free__ = function
      @__state__ ||= [__address__, nil, false]
      @__state__[1] = function
      ObjectSpace.define_finalizer(self, Pointer.__finalizer__(@__state__)) if function && !had
      nil
    end

    def free
      return nil unless @__free__
      Function.new(Fiddle.__signed__(@__free__), [Types::VOIDP], Types::VOID)
    end

    def call_free
      Pointer.__release__(@__state__) if @__state__
      nil
    end

    def freed? = @__state__ ? @__state__[2] : false

    def to_i = Fiddle.__signed__(__address__)
    alias to_int to_i

    def to_value = Fiddle.dlunwrap(to_i)

    # The pointer stored at this address.
    def ptr = Pointer.new(Fiddle.__signed__(Spinel::FFI.read(__address__, Fiddle::SIZEOF_VOIDP).unpack1("J")))
    alias +@ ptr

    # A pointer to this pointer's address.
    def ref
      cell = Spinel::FFI.malloc(Fiddle::SIZEOF_VOIDP)
      Spinel::FFI.write(cell, [__address__].pack("J"))
      pointer = Pointer.allocate
      pointer.__set__(cell, 0, nil)
      pointer.instance_variable_set(:@__cell__, Pointer.new(cell, 0, Spinel::FFI::FREE))
      pointer
    end
    alias -@ ref

    def null? = __address__.zero?

    def to_s(length = nil)
      return Spinel::FFI.string(__address__) if length.nil?
      Spinel::FFI.read(__address__, Spinel::FFI.int(length))
    end

    def to_str(length = nil)
      Spinel::FFI.read(__address__, length.nil? ? size : Spinel::FFI.int(length))
    end

    def inspect
      format("#<%s:0x%016x ptr=0x%016x size=%d free=0x%016x>",
        self.class, __id__ * 8, __address__, size, @__free__ || 0)
    end

    def <=>(other)
      return nil unless Pointer === other
      __address__ <=> other.__address__
    end

    def ==(other) = Pointer === other && __address__ == other.__address__
    alias eql? ==

    def +(offset)
      offset = Spinel::FFI.long(offset)
      Pointer.allocate.__set__((__address__ + offset) & 0xFFFF_FFFF_FFFF_FFFF, size - offset, nil)
    end

    def -(offset)
      offset = Spinel::FFI.long(offset)
      Pointer.allocate.__set__((__address__ - offset) & 0xFFFF_FFFF_FFFF_FFFF, size + offset, nil)
    end

    def [](offset, length = (single = true; nil))
      raise DLError, "NULL pointer dereference" if null?
      offset = Spinel::FFI.ulong(offset)
      at = (__address__ + offset) & 0xFFFF_FFFF_FFFF_FFFF
      if single
        Spinel::FFI.read(at, 1).unpack1("c")
      else
        Spinel::FFI.read(at, Spinel::FFI.ulong(length))
      end
    end

    def []=(offset, *rest)
      raise DLError, "NULL pointer dereference" if null?
      unless rest.size.between?(1, 2)
        raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 2..3)"
      end
      at = (__address__ + Spinel::FFI.ulong(offset)) & 0xFFFF_FFFF_FFFF_FFFF
      if rest.size == 1
        byte = rest[0]
        Spinel::FFI.write(at, [Spinel::FFI.uint(byte)].pack("C"))
        return byte
      end
      length, source = rest
      length = Spinel::FFI.ulong(length)
      bytes =
        if String === source
          # Past the String's end CRuby reads whatever follows it; this reads
          # zeros instead.
          source.b.byteslice(0, length).ljust(length, "\0")
        elsif Pointer === source
          Spinel::FFI.read(source.__address__, length)
        else
          Spinel::FFI.read(Fiddle.__address__(source), length)
        end
      Spinel::FFI.write(at, bytes)
      source
    end

    def size = @__size__ || 0

    def size=(size)
      @__size__ = Spinel::FFI.long(size)
      size
    end

    def __address__ = @__address__ || 0
  end

  NULL = Pointer.new(0)

  # --- Fiddle::Function (function.c) ------------------------------------------

  class Function
    DEFAULT = Spinel::FFI::DEFAULT_ABI

    def initialize(ptr, args, ret_type, abi = nil, name: nil, need_gvl: false)
      @closure = ptr
      @name = String === name ? -name : name
      @need_gvl = need_gvl
      @ptr = Fiddle.__signed__(Fiddle.__address__(ptr))
      @abi = abi.nil? ? DEFAULT : Spinel::FFI.int(abi)
      @return_type = Spinel::FFI.check_type(Fiddle.__type__(ret_type))
      unless Array === args
        raise TypeError, "wrong argument type #{args.class} (expected Array)"
      end
      types = []
      variadic = false
      args.each_with_index do |arg, index|
        code = Fiddle.__type__(arg)
        if code == Types::VARIADIC
          if index != args.size - 1
            raise ArgumentError, "Fiddle::TYPE_VARIADIC must be the last argument type: #{args.inspect}"
          end
          variadic = true
          break
        end
        types << Spinel::FFI.check_type(code)
      end
      @argument_types = types.freeze
      @is_variadic = variadic
    end

    def call(*args)
      fixed = @argument_types.size
      if @is_variadic
        if args.size < fixed
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected #{fixed}+)"
        end
        if (args.size - fixed).odd?
          raise ArgumentError, "variadic arguments must be type and value pairs: #{args.inspect}"
        end
        types = @argument_types.dup
        values = args[0, fixed]
        index = fixed
        while index < args.size
          types << Spinel::FFI.check_type(Fiddle.__type__(args[index]))
          values << args[index + 1]
          index += 2
        end
      else
        if args.size != fixed
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected #{fixed})"
        end
        types = @argument_types
        values = args.dup
      end
      buffers = []
      copies = []
      kept = []
      begin
        types.each_with_index do |type, index|
          value = values[index]
          case type
          when Types::VOIDP
            if value.nil?
              values[index] = 0
            elsif String === value && !(Pointer === value)
              address = Spinel::FFI.malloc(value.bytesize + 1)
              Spinel::FFI.write(address, value.b) unless value.empty?
              buffers << address
              copies << [value, address]
              values[index] = address
            else
              value = Pointer[value] unless value.class == Pointer
              kept << value
              values[index] = Fiddle.__address__(value)
            end
          when Types::CONST_STRING
            if value.nil?
              values[index] = 0
            else
              address = Fiddle.__c_string__(value)
              buffers << address
              values[index] = address
            end
          end
        end
        result, errno = Spinel::FFI.call(@ptr & 0xFFFF_FFFF_FFFF_FFFF, types, @return_type, values, @abi, fixed)
        copies.each do |string, address|
          next if string.frozen? || string.empty?
          written = Spinel::FFI.read(address, string.bytesize)
          string.replace(written.force_encoding(string.encoding)) unless written == string.b
        end
      ensure
        buffers.each { |address| Spinel::FFI.free(address) }
      end
      Fiddle.last_error = errno
      Function.__value__(@return_type, result)
    end

    # `rb_fiddle_generic_to_value`.
    def self.__value__(type, value)
      case type
      when Types::VOIDP then Pointer.new(Fiddle.__signed__(value))
      when Types::CONST_STRING then value.zero? ? nil : Spinel::FFI.string(value)
      when Types::BOOL then value != 0
      when -Types::LONG, -Types::LONG_LONG, -Types::INT, -Types::SHORT, -Types::CHAR
        value
      else value
      end
    end
  end

  # --- Fiddle::Closure (closure.c) --------------------------------------------

  class Closure
    @live = ObjectSpace::WeakMap.new

    def self.__finalizer__(id)
      proc { Spinel::FFI.free_closure(id) }
    end

    # The block `Spinel::FFI` runs for a closure: made here, not in
    # `initialize`, so that it does not hold the Closure and the Closure can
    # be collected (and its C closure freed) when nothing else holds it.
    def self.__handler__(holder)
      proc { |*raw| Closure.__dispatch__(holder[0], raw) }
    end

    # Run the closure `id` with C's arguments, as `with_gvl_callback` does.
    def self.__dispatch__(id, raw)
      closure = @live[id]
      raise RuntimeError, "a collected Fiddle::Closure was called" unless closure
      closure.__call_from_c__(raw)
    end

    def initialize(ret, args, abi = Function::DEFAULT)
      unless Array === args
        raise TypeError, "wrong argument type #{args.class} (expected Array)"
      end
      types = args.map { |arg| Spinel::FFI.check_type(Fiddle.__type__(arg)) }.freeze
      @ctype = Spinel::FFI.check_type(Fiddle.__type__(ret))
      @args = types
      holder = []
      made = Spinel::FFI.closure(@ctype, types.take_while { |t| t != Types::VOID },
        Spinel::FFI.int(abi), &Closure.__handler__(holder))
      holder[0] = made[0]
      @__id__ = made[0]
      @__code__ = made[1]
      Closure.instance_variable_get(:@live)[made[0]] = self
      ObjectSpace.define_finalizer(self, Closure.__finalizer__(made[0]))
    end

    def __call_from_c__(raw)
      params = []
      @args.each_with_index do |type, index|
        break if type == Types::VOID
        value = raw[index]
        params <<
          case type
          when Types::VOIDP then Pointer[Fiddle.__signed__(value)]
          when Types::CONST_STRING then value.zero? ? nil : Spinel::FFI.string(value)
          when Types::BOOL then value != 0
          else value
          end
      end
      result = call(*params)
      case @ctype
      when Types::VOID then nil
      when Types::VOIDP then Fiddle.__address__(result)
      when Types::CONST_STRING
        # C reads the String after the callback returns, so the copy stays
        # until the next one replaces it or the closure is freed.
        Spinel::FFI.free(@__string__) if @__string__
        @__string__ = Fiddle.__c_string__(result)
      else result
      end
    end

    def to_i
      raise ArgumentError, "already freed: #{inspect}" if freed?
      Fiddle.__signed__(@__code__)
    end

    def free
      return nil if freed?
      Spinel::FFI.free_closure(@__id__)
      Closure.instance_variable_get(:@live).delete(@__id__)
      Spinel::FFI.free(@__string__) if @__string__
      @__string__ = nil
      @__id__ = nil
      nil
    end

    def freed? = @__id__.nil?
  end

  # --- Fiddle::Pinned (pinned.c) ----------------------------------------------

  class Pinned
    CLEARED = Object.new
    private_constant :CLEARED

    def initialize(object)
      @object = object
    end

    def ref
      raise ClearedReferenceError, "`ref` called on a cleared object" if cleared?
      @object
    end

    def clear
      @object = CLEARED
      self
    end

    def cleared? = CLEARED.equal?(@object)
  end
end

