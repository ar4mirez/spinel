# frozen_string_literal: true

# `ossl_bn.c` on Spinel (#107): `OpenSSL::BN`, a big number. CRuby's wraps
# a `BIGNUM`; this one wraps an Integer, which is a big number already, so
# the arithmetic is Ruby's. Only a primality test and prime generation ask
# OpenSSL (`BN_check_prime`, `BN_generate_prime_ex`), so they answer as its
# do.
#
# ponytail: `BN#get_flags`/`set_flags` and `CONSTTIME`'s effect are not
# here (an Integer has no constant-time mode), nor `mod_sqrt` and
# `mod_inverse` on numbers with no inverse past what `pow` says. Upgrade:
# the `BN_*` calls behind `__openssl_tls__`.

module OpenSSL
  class BNError < OpenSSLError; end

  class BN
    include Comparable

    CONSTTIME = 4

    # Made in C on CRuby, so `Marshal.dump` refuses one.
    ::Marshal.__opaque_class__(self)

    def self.__int__(value)
      case value
      when ::Integer then value
      when BN then value.to_i
      when nil then raise TypeError, "Cannot convert nil into OpenSSL::BN"
      else
        raise TypeError, "Cannot convert into OpenSSL::BN" unless value.respond_to?(:to_int)
        Integer.__index__(value)
      end
    end
    private_class_method :__int__

    # :call-seq:
    #    OpenSSL::BN.new(bn) -> aBN
    #    OpenSSL::BN.new(integer) -> aBN
    #    OpenSSL::BN.new(string, base = 10) -> aBN
    #
    # Construct a new \OpenSSL BIGNUM object. A String is read in _base_: 10
    # and 16 as digits, 2 as big-endian bytes of the absolute value, and 0
    # as an MPI (a 4-byte length, then the bytes, the top bit the sign).
    def initialize(*args)
      unless args.size == 1 || args.size == 2
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
      end
      value, base = args
      base = base.nil? ? 10 : Integer.__index__(base)
      if args.size == 1 && (::Integer === value || BN === value)
        @__n__ = value.to_i
        return
      end
      if args.size == 1 && !(String === value) && value.respond_to?(:to_int)
        @__n__ = Integer.__index__(value)
        return
      end
      str = IO.__str__(value)
      @__n__ =
        case base
        when 10 then BN.__send__(:__parse__, str, 10)
        when 16 then BN.__send__(:__parse__, str, 16)
        when 2 then str.unpack1("H*").then { |h| h.empty? ? 0 : h.to_i(16) }
        when 0 then BN.__send__(:__mpi__, str)
        else raise ArgumentError, "invalid radix #{base}"
        end
    end

    # `BN_dec2bn` and `BN_hex2bn`, which take an optional `-` and then as
    # many digits as there are.
    def self.__parse__(str, base)
      digits = base == 10 ? /\A-?[0-9]+/ : /\A-?[0-9a-fA-F]+/
      m = digits.match(str)
      raise BNError, base == 10 ? "BN_dec2bn" : "BN_hex2bn" if m.nil? || m[0] == "-"
      m[0].to_i(base)
    end
    private_class_method :__parse__

    # `BN_mpi2bn`: a 4-byte length, then the bytes, the top bit the sign.
    def self.__mpi__(str)
      raise BNError, "invalid length" if str.bytesize < 4
      len = str.unpack1("N")
      body = str.byteslice(4, len) || "".b
      raise BNError, "encoding error" unless body.bytesize == len
      return 0 if len.zero?
      n = body.unpack1("H*").to_i(16)
      return n if body.getbyte(0) & 0x80 == 0
      -(n - (1 << (8 * len - 1)))
    end
    private_class_method :__mpi__

    def initialize_copy(other)
      super
      @__n__ = other.to_i
      self
    end

    def copy(other)
      @__n__ = BN.__send__(:__int__, other)
      self
    end

    # :call-seq:
    #    bn.to_s(base = 10) -> string
    def to_s(base = 10)
      base = Integer.__index__(base)
      n = @__n__
      case base
      when 10 then n.to_s
      when 16
        hex = n.abs.to_s(16).upcase
        hex = "0#{hex}" if hex.size.odd?
        n.negative? ? "-#{hex}" : hex
      when 2
        hex = n.abs.to_s(16)
        hex = "0#{hex}" if hex.size.odd?
        n.zero? ? "".b : [hex].pack("H*")
      when 0
        abs = n.abs
        hex = abs.zero? ? "" : abs.to_s(16)
        hex = "0#{hex}" if hex.size.odd?
        body = [hex].pack("H*")
        body = "\x00".b + body if !body.empty? && body.getbyte(0) & 0x80 != 0
        body.setbyte(0, body.getbyte(0) | 0x80) if n.negative?
        [body.bytesize].pack("N") + body
      else raise ArgumentError, "invalid radix #{base}"
      end
    end

    def to_i = @__n__
    alias to_int to_i

    def to_bn = self

    def inspect = Object.instance_method(:to_s).bind_call(self)

    def coerce(other)
      case other
      when String then [other, to_s]
      when ::Integer then [other, to_i]
      else raise TypeError, "Don't know how to coerce"
      end
    end

    def zero? = @__n__.zero?
    def one? = @__n__ == 1
    def odd? = @__n__.odd?
    def negative? = @__n__.negative?

    def num_bits = @__n__.abs.bit_length
    def num_bytes = (num_bits + 7) / 8

    def +(other) = BN.new(@__n__ + BN.__send__(:__int__, other))
    def -(other) = BN.new(@__n__ - BN.__send__(:__int__, other))
    def *(other) = BN.new(@__n__ * BN.__send__(:__int__, other))
    def sqr = BN.new(@__n__ * @__n__)
    def -@ = BN.new(-@__n__)
    def +@ = self
    def abs = BN.new(@__n__.abs)

    # `BN_div`: the quotient truncated toward zero, and the remainder with
    # the dividend's sign.
    def /(other)
      d = BN.__send__(:__int__, other)
      raise BNError, "div by zero" if d.zero?
      q = @__n__.abs / d.abs
      q = -q if (@__n__ < 0) ^ (d < 0)
      [BN.new(q), BN.new(@__n__ - q * d)]
    end

    # `BN_mod`: the remainder with the dividend's sign.
    def %(other)
      d = BN.__send__(:__int__, other)
      raise BNError, "div by zero" if d.zero?
      r = @__n__.abs % d.abs
      BN.new(@__n__ < 0 ? -r : r)
    end

    def **(other)
      e = BN.__send__(:__int__, other)
      raise BNError, "BN_exp" if e.negative?
      BN.new(@__n__**e)
    end

    def gcd(other) = BN.new(@__n__.gcd(BN.__send__(:__int__, other)))

    def mod_add(b, m) = BN.new((@__n__ + BN.__send__(:__int__, b)) % BN.__send__(:__int__, m))
    def mod_sub(b, m) = BN.new((@__n__ - BN.__send__(:__int__, b)) % BN.__send__(:__int__, m))
    def mod_mul(b, m) = BN.new((@__n__ * BN.__send__(:__int__, b)) % BN.__send__(:__int__, m))
    def mod_sqr(m) = BN.new((@__n__ * @__n__) % BN.__send__(:__int__, m))

    def mod_exp(e, m)
      BN.new(@__n__.pow(BN.__send__(:__int__, e), BN.__send__(:__int__, m)))
    end

    def mod_inverse(m)
      m = BN.__send__(:__int__, m)
      BN.new(@__n__.pow(-1, m))
    rescue Math::DomainError, ZeroDivisionError
      raise BNError, "BN_mod_inverse: no inverse"
    end

    def <<(bits) = BN.new(@__n__ << Integer.__index__(bits))
    def >>(bits) = BN.new(@__n__ >> Integer.__index__(bits))

    def lshift!(bits)
      @__n__ <<= Integer.__index__(bits)
      self
    end

    def rshift!(bits)
      @__n__ >>= Integer.__index__(bits)
      self
    end

    def set_bit!(bit)
      @__n__ |= (1 << Integer.__index__(bit))
      self
    end

    def clear_bit!(bit)
      @__n__ &= ~(1 << Integer.__index__(bit))
      self
    end

    def mask_bits!(bits)
      @__n__ &= (1 << Integer.__index__(bits)) - 1
      self
    end

    def bit_set?(bit) = @__n__[Integer.__index__(bit)] == 1

    def cmp(other) = @__n__ <=> BN.__send__(:__int__, other)
    alias <=> cmp

    def ucmp(other) = @__n__.abs <=> BN.__send__(:__int__, other).abs

    def ==(other)
      case other
      when BN, ::Integer then @__n__ == other.to_i
      else false
      end
    end
    alias === ==

    def eql?(other) = BN === other && @__n__ == other.to_i

    def hash = [BN, @__n__].hash

    # :call-seq:
    #    bn.prime? => true | false
    #    bn.prime?(checks) => true | false
    def prime?(checks = nil)
      unless checks.nil?
        warn("OpenSSL::BN#prime? with an argument is deprecated", uplevel: 1, category: :deprecated)
      end
      return false if @__n__ < 2
      r = __openssl_tls__("bn_prime", to_s(16))
      raise OpenSSL.__send__(:__error__, r[1]) if Array === r && r[0] == :__failure__
      r
    end

    def prime_fasttest?(checks = nil, trial_div = true) = prime?

    def get_flags(flags) = 0
    def set_flags(flags) = nil

    class << self
      # :call-seq:
      #    BN.generate_prime(bits, [, safe [, add [, rem]]]) => bn
      def generate_prime(bits, safe = true, add = nil, rem = nil)
        unless add.nil? && rem.nil?
          raise NotImplementedError, "OpenSSL::BN.generate_prime with add and rem is not supported on Spinel"
        end
        r = __openssl_tls__("bn_generate_prime", Integer.__index__(bits), safe ? true : false)
        raise OpenSSL.__send__(:__error__, r[1]) if Array === r && r[0] == :__failure__
        BN.new(r, 16)
      end

      # :call-seq:
      #    BN.rand(bits [, fill [, odd]]) -> aBN
      def rand(bits, fill = 0, odd = false)
        bits = Integer.__index__(bits)
        raise BNError, "bits too small" if bits.negative?
        return BN.new(0) if bits.zero?
        bytes = OpenSSL::Random.random_bytes((bits + 7) / 8)
        n = bytes.unpack1("H*").to_i(16) & ((1 << bits) - 1)
        case fill
        when 0 then n |= 1 << (bits - 1)
        when 1 then n |= 3 << (bits - 2) if bits > 1
        end
        n |= 1 if odd
        BN.new(n)
      end
      alias pseudo_rand rand

      # :call-seq:
      #    BN.rand_range(range) -> aBN
      def rand_range(range)
        r = BN.__send__(:__int__, range)
        raise BNError, "invalid range" unless r.positive?
        bytes = OpenSSL::Random.random_bytes((r.bit_length + 15) / 8)
        BN.new(bytes.unpack1("H*").to_i(16) % r)
      end
      alias pseudo_rand_range rand_range
    end
  end
end
