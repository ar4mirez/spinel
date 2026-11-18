# `Spinel::FFI` (#105): load a C library, look up a function, call it.
#
# The escape hatch for code that only needs to call C: libffi under Ruby,
# with no extension to build. `fiddle` is this module's first user
# (`lib/fiddle/spinel.rb`); its API is the one most programs want. What is
# here is the layer under it: C types by Fiddle's type codes, integers and
# floats converted as CRuby's `NUM2INT` and friends convert them, addresses as
# Integers, and closures — C function pointers that call a Ruby block.
#
# The C half is `spinel-vm/src/ffi.rs`, the `__ffi_*__` primitives. A value
# crosses to it as an 8-byte slot: an integer's bits, sign- or zero-extended
# from its C type, or a Float's. A C call that calls a closure comes back here
# part-way through (`[1, call, closure, slots]`); the block runs as any other
# Ruby does, and its answer goes back to the waiting C call through
# `__ffi_resume__`. A block that raises, breaks or throws abandons the C call,
# as CRuby's `longjmp` out of a callback abandons the C frames under it.
#
#   libc = Spinel::FFI.open(nil)
#   strlen = Spinel::FFI.symbol(libc, "strlen")
#   name = Spinel::FFI.malloc(6)
#   Spinel::FFI.write(name, "hello\0")
#   Spinel::FFI.call(strlen, [Spinel::FFI::VOIDP], -Spinel::FFI::LONG, [name]) # => [5, errno]

module Spinel
  module FFI
    # Raised for a library or symbol that cannot be loaded, and a handle
    # that will not close: `dlerror`'s message.
    class Error < StandardError; end

    VOID = 0
    VOIDP = 1
    CHAR = 2
    SHORT = 3
    INT = 4
    LONG = 5
    LONG_LONG = 6
    FLOAT = 7
    DOUBLE = 8
    VARIADIC = 9
    CONST_STRING = 10
    BOOL = 11

    __ffi_constants__.each { |name, value| const_set(name, value) }

    @closures = {}

    # The C type names `rb_fiddle_type_ensure` reads, as type codes.
    NAMES = {
      void: VOID, voidp: VOIDP, char: CHAR, short: SHORT, int: INT,
      long: LONG, long_long: LONG_LONG, int8_t: CHAR, int16_t: SHORT,
      int32_t: INT, int64_t: LONG, float: FLOAT, double: DOUBLE,
      variadic: VARIADIC, const_string: CONST_STRING, size_t: -LONG,
      ssize_t: LONG, ptrdiff_t: LONG, intptr_t: LONG, uintptr_t: -LONG,
      bool: BOOL,
    }.freeze

    class << self
      # --- libraries --------------------------------------------------------

      # `dlopen(3)` the library at `path`, nil for the program and the
      # libraries it already has: the handle, an Integer.
      def open(path, flags = RTLD_LAZY | RTLD_GLOBAL)
        handle = __ffi_dlopen__(path, flags)
        raise Error, handle if String === handle
        handle
      end

      # The address of `name` in the library `handle`, nil if it has none.
      def symbol(handle, name) = __ffi_dlsym__(handle, name)

      # `dlclose(3)` `handle`.
      def close(handle)
        failure = __ffi_dlclose__(handle)
        raise Error, failure if failure
        nil
      end

      # --- memory -------------------------------------------------------------

      # `size` zeroed bytes of C memory: their address.
      def malloc(size)
        address = __ffi_malloc__(size)
        raise NoMemoryError, "failed to allocate memory" if address.zero?
        address
      end

      def realloc(address, size)
        moved = __ffi_realloc__(address, size)
        raise NoMemoryError, "failed to allocate memory" if moved.zero? && size > 0
        moved
      end

      def free(address) = __ffi_free__(address)

      # A BINARY copy of the `length` bytes at `address`.
      def read(address, length) = __ffi_read__(address, length)

      # Copy `bytes` to `address`.
      def write(address, bytes) = __ffi_write__(address, bytes)

      # The length of the C string at `address`.
      def strlen(address) = __ffi_strlen__(address)

      # The C string at `address`, BINARY.
      def string(address) = __ffi_read__(address, __ffi_strlen__(address))

      # --- types --------------------------------------------------------------

      # A type code for `type`: a code, or a C type's name as a Symbol or
      # String, as `rb_fiddle_type_ensure` takes it.
      def type(type)
        original = type
        type = type.to_str.to_sym if !(Symbol === type) && type.respond_to?(:to_str)
        if Symbol === type
          code = NAMES[type]
          return code if code
          type = original
        end
        Integer.__index__(type)
      end

      # Bytes a value of type `code` takes, and its alignment.
      def sizeof(code) = (__ffi_layout__(code) || raise(ArgumentError, "unknown type #{code}"))[0]
      def alignof(code) = (__ffi_layout__(code) || raise(ArgumentError, "unknown type #{code}"))[1]

      # Raise as `fiddle`'s conversions do for a code that names no type.
      def check_type(code)
        return code if code == VOID || __ffi_layout__(code)
        raise RuntimeError, "unknown type #{code}"
      end

      # --- calls --------------------------------------------------------------

      # Call the function at `address`, taking `types` and returning
      # `return_type`, with `args`: its return value and the `errno` it left.
      # With `fixed`, the arguments past the first `fixed` are variadic.
      #
      # An integer argument is converted as CRuby converts one for its C
      # type, a floating-point one by `Float()`, a pointer is an address, and
      # a bool is the argument's truth.
      def call(address, types, return_type, args, abi = DEFAULT_ABI, fixed = nil)
        slots = pack(types, args)
        outcome = __ffi_call__(address, abi, types, fixed || types.size, return_type, slots)
        outcome = __answer__(outcome) while outcome[0] == 1
        [unpack(return_type, outcome[1]), outcome[2]]
      end

      # A closure: a C function taking `types` and returning `return_type`
      # that calls `block` with its arguments, as `call` would answer them,
      # and returns what the block answers, converted as `call` converts an
      # argument. Its id, for `free_closure`, and the address C calls.
      def closure(return_type, types, abi = DEFAULT_ABI, &block)
        made = __ffi_closure__(return_type, types, abi)
        raise RuntimeError, made if String === made
        @closures[made[0]] = [return_type, types.dup.freeze, block]
        made
      end

      # Free the closure `id`. Its address must not be called again.
      def free_closure(id)
        @closures.delete(id)
        __ffi_closure_free__(id)
        nil
      end

      # --- conversions --------------------------------------------------------

      # `args` as slots for `types`.
      def pack(types, args)
        slots = "".b
        i = 0
        while i < types.size
          slots << [__slot__(types[i], args[i])].pack("Q")
          i += 1
        end
        slots
      end

      # The value of type `code` in `slot`.
      def unpack(code, slot)
        case code.abs
        when VOID then nil
        when FLOAT, DOUBLE then slot.unpack1("d")
        else code < 0 || code == BOOL ? slot.unpack1("Q") : slot.unpack1("q")
        end
      end

      # `rb_num2long`'s coercion: an Integer, a Float in range, or what
      # `#to_int` answers.
      def long(value, type = "long")
        integer =
          case value
          when Integer then value
          when Float then __float__(value, 9_223_372_036_854_775_808.0)
          when nil then raise TypeError, "no implicit conversion from nil to integer"
          else Integer.__index__(value)
          end
        unless integer.between?(-0x8000_0000_0000_0000, 0x7FFF_FFFF_FFFF_FFFF)
          raise RangeError, "bignum too big to convert into '#{type}'"
        end
        integer
      end

      # `rb_num2ulong`: as `long`, from `-2**63` to `2**64 - 1`, wrapped.
      def ulong(value, type = "unsigned long")
        integer =
          case value
          when Integer then value
          when Float then __float__(value, 18_446_744_073_709_551_616.0)
          when nil then raise TypeError, "no implicit conversion of nil into Integer"
          else Integer.__index__(value)
          end
        raise RangeError, "bignum too big to convert into '#{type}'" if integer > 0xFFFF_FFFF_FFFF_FFFF
        raise RangeError, "bignum out of range of #{type}" if integer < -0x8000_0000_0000_0000
        integer & 0xFFFF_FFFF_FFFF_FFFF
      end

      # `NUM2INT`.
      def int(value)
        integer = long(value)
        raise RangeError, "integer #{integer} too big to convert to 'int'" if integer > 0x7FFF_FFFF
        raise RangeError, "integer #{integer} too small to convert to 'int'" if integer < -0x8000_0000
        integer
      end

      # `NUM2UINT`: -2**31 to 2**32 - 1, wrapped.
      def uint(value)
        integer = ulong(value)
        signed = integer >= 0x8000_0000_0000_0000 ? integer - 0x1_0000_0000_0000_0000 : integer
        if signed < 0
          raise RangeError, "integer #{signed} too small to convert to 'unsigned int'" if signed < -0x8000_0000
        elsif integer > 0xFFFF_FFFF
          raise RangeError, "integer #{integer} too big to convert to 'unsigned int'"
        end
        integer & 0xFFFF_FFFF
      end

      # `NUM2DBL`.
      def double(value)
        case value
        when Float then value
        when Integer then value.to_f
        when String then raise TypeError, "no implicit conversion to float from string"
        when nil then raise TypeError, "no implicit conversion to float from nil"
        when true then raise TypeError, "no implicit conversion to float from true"
        when false then raise TypeError, "no implicit conversion to float from false"
        when Numeric then value.to_f
        else raise TypeError, "can't convert #{value.class} into Float"
        end
      end

      private

      def __float__(value, upper)
        unless value.finite? && value >= -9_223_372_036_854_775_808.0 && value < upper
          raise RangeError, "float #{format("%-.10g", value)} out of range of integer"
        end
        value.to_i
      end

      # `value` as the slot for an argument of type `code`.
      def __slot__(code, value)
        case code
        when VOIDP, CONST_STRING then ulong(value)
        when CHAR
          return value.getbyte(0) & 0xFF if String === value && value.bytesize == 1
          int(value) & 0xFFFF_FFFF_FFFF_FFFF
        when SHORT, INT then int(value) & 0xFFFF_FFFF_FFFF_FFFF
        when -CHAR, -SHORT, -INT then uint(value)
        when LONG then long(value) & 0xFFFF_FFFF_FFFF_FFFF
        when -LONG then ulong(value)
        when LONG_LONG then long(value, "long long") & 0xFFFF_FFFF_FFFF_FFFF
        when -LONG_LONG then ulong(value, "unsigned long long")
        when FLOAT, DOUBLE then [double(value)].pack("d").unpack1("Q")
        when BOOL then value ? 1 : 0
        when VOID then 0
        else raise RuntimeError, "unknown type #{code}"
        end
      end

      # Run the closure a C call is waiting on, and hand its answer back to
      # the call: the call's next outcome.
      def __answer__(outcome)
        _, call, id, slots = outcome
        answered = false
        begin
          return_type, types, block = @closures[id]
          raise RuntimeError, "a freed Fiddle closure was called" unless block
          args = []
          i = 0
          while i < types.size
            args << unpack(types[i], slots.byteslice(i * 8, 8))
            i += 1
          end
          answer = [__slot__(return_type, block.call(*args))].pack("Q")
          answered = true
        ensure
          __ffi_abandon__(call) unless answered
        end
        __ffi_resume__(call, answer)
      end
    end
  end
end
