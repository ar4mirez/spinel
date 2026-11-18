# frozen_string_literal: true

# `openssl`'s C half on Spinel (#106), phase 1: digests, HMAC and random
# bytes. `require "openssl"` is ruby/openssl's own `openssl.rb` (vendored in
# `stdlib/` at the release Ruby 4.0 bundles), which requires this in place of
# `openssl.so` and then the Ruby half of each part whose C half is here.
#
# What is here is `ext/openssl` function by function: `ossl.c`'s module,
# version constants, `OpenSSLError` and `fixed_length_secure_compare`;
# `ossl_digest.c`'s `OpenSSL::Digest`; `ossl_hmac.c`'s `OpenSSL::HMAC`; and
# `ossl_rand.c`'s `OpenSSL::Random`. Each is Ruby over the `__openssl_*__`
# primitives (`spinel-vm/src/openssl.rs`), which make the same OpenSSL calls
# the C does, on the system's OpenSSL 3, and answer a failure as the
# `[class, message, errors]` that `ossl_raise` would have raised.
#
# Everything else in `ext/openssl` — `Cipher`, `PKey`, `X509`, `SSL`, `BN`,
# `KDF`, `ASN1` and the rest — is not here, and neither is anything that
# would stand in for it: those constants are undefined until their phase
# lands (TLS is #107), and `openssl.rb` skips their Ruby halves.
#
# ponytail: `OpenSSL.debug` and `debug=` are not here either. With debug on,
# CRuby warns about every OpenSSL error it collects, including the ones it
# clears while looking a digest up by name, which `__openssl_digest_init__`
# does not hand back. Upgrade: have the primitives answer the errors they
# clear, and warn about them here.
#
# A digest's or HMAC's OpenSSL context lives in the heap, under an id the
# object holds in a one-element Array, the *holder*. The holder carries the
# finalizer that frees the context, so `dup` and `clone`, which copy the
# object's finalizers and instance variables, never leave two objects
# freeing one context: `initialize_copy` gives the copy a holder of its own.

require "digest"

module OpenSSL
  version, number = __openssl_version__
  OPENSSL_VERSION = version.freeze
  OPENSSL_LIBRARY_VERSION = version.dup.freeze
  OPENSSL_VERSION_NUMBER = number
  # OpenSSL 3 always has a FIPS provider to load, so CRuby defines this
  # true whenever it is built against one; `build.rs` requires OpenSSL 3.
  OPENSSL_FIPS = true

  class OpenSSLError < StandardError
    attr_reader :errors

    # The message, and then each OpenSSL error that was queued when this was
    # raised, as `osslerror_detailed_message` lists them.
    def detailed_message(**)
      text = super
      errors = @errors
      return text unless Array === errors
      text = text.dup
      text << "\nOpenSSL error queue reported #{errors.length} errors:"
      errors.each { |error| text << "\n" << error }
      text
    end
  end

  # :call-seq:
  #    OpenSSL.fixed_length_secure_compare(string, string) -> true or false
  #
  # Constant time memory comparison for fixed length strings, such as
  # results of HMAC calculations.
  def self.fixed_length_secure_compare(a, b)
    a = IO.__str__(a)
    b = IO.__str__(b)
    raise ArgumentError, "inputs must be of equal length" unless a.bytesize == b.bytesize
    __openssl_memcmp__(a, b)
  end

  # The exception a primitive's failure, `[class, message, errors]`,
  # stands for. The OpenSSL error classes carry the queued errors, as
  # `ossl_make_error` leaves them in `@errors`.
  def self.__error__(failure)
    code, message, errors = failure
    case code
    when 4 then return ArgumentError.new(message)
    when 5 then return RangeError.new(message)
    when 6 then return RuntimeError.new(message)
    end
    klass =
      case code
      when 1 then Digest::DigestError
      when 2 then HMACError
      else Random::RandomError
      end
    error = klass.new(message)
    error.instance_variable_set(:@errors, errors)
    error
  end

  # A holder for context `id`: the Array the object keeps, with the
  # finalizer that frees the context once nothing keeps the holder.
  #
  # ponytail: the finalizer is registered with `__finalizer_add__`, the
  # primitive `ObjectSpace.define_finalizer` ends in, not through
  # `define_finalizer` itself, whose check that a block does not capture
  # its object asks the block for its `binding`; a `Proc#binding` keeps
  # some 600 bytes the collector never frees, so every digest would leak
  # that much. The block here is made in `__free__` and cannot capture the
  # holder. Upgrade: call `define_finalizer` once `Proc#binding` frees what
  # it makes.
  def self.__holder__(id)
    holder = [id]
    __finalizer_add__(holder, __free__(id))
    holder
  end

  # Made here, so the finalizer holds the id and not the holder.
  def self.__free__(id) = proc { __openssl_free__(id) }

  # `ossl_evp_md_fetch`'s argument: a digest's context id, or a name.
  def self.__algorithm__(value)
    if Digest === value
      holder = value.instance_variable_get(:@__ctx__)
      raise RuntimeError, "Digest CTX wasn't initialized!" if holder.nil?
      return holder[0]
    end
    __cstr__(value)
  end

  # `StringValueCStr`: a String, without a NUL in it.
  def self.__cstr__(value)
    value = IO.__str__(value)
    raise ArgumentError, "string contains null byte" if value.include?("\0")
    value
  end

  # `NUM2INT`.
  def self.__int__(value)
    if Float === value
      unless value.finite? && value > -9_223_372_036_854_775_809.0 && value < 9_223_372_036_854_775_808.0
        raise RangeError, "float #{format("%-.10g", value)} out of range of integer"
      end
      value = value.to_i
    else
      value = Integer.__index__(value)
    end
    if value > 0x7FFF_FFFF_FFFF_FFFF || value < -0x8000_0000_0000_0000
      raise RangeError, "bignum too big to convert into 'long'"
    end
    raise RangeError, "integer #{value} too big to convert to 'int'" if value > 0x7FFF_FFFF
    raise RangeError, "integer #{value} too small to convert to 'int'" if value < -0x8000_0000
    value
  end

  # `NUM2DBL`.
  def self.__double__(value)
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

  private_class_method :__error__, :__holder__, :__free__, :__algorithm__, :__cstr__,
    :__int__, :__double__

  # ossl_digest.c
  class Digest < ::Digest::Class
    class DigestError < OpenSSLError; end

    # Made in C on CRuby, so `Marshal.dump` refuses one.
    Marshal.__opaque_class__(self)

    # :call-seq:
    #    OpenSSL::Digest.digests -> array[string...]
    #
    # Returns the names of all available digests in an array.
    def self.digests = __openssl_digests__

    def digests = __openssl_digests__
    private :digests

    # :call-seq:
    #    Digest.new(string [, data]) -> Digest
    #
    # Creates a Digest instance based on _string_, which is either the ln
    # (long name) or sn (short name) of a supported digest algorithm, an
    # OID, or the name of an algorithm a provider offers.
    # Any number of arguments, as a C function's `rb_scan_args` takes them,
    # so the arity is CRuby's -1.
    def initialize(*args)
      unless args.size == 1 || args.size == 2
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
      end
      name, data = args
      algorithm = OpenSSL.__send__(:__algorithm__, name)
      data = IO.__str__(data) unless data.nil?
      holder = @__ctx__
      id = __openssl_digest_init__(holder&.first, algorithm)
      raise OpenSSL.__send__(:__error__, id) if Array === id
      @__ctx__ = OpenSSL.__send__(:__holder__, id) unless holder&.first == id
      __update__(data) unless data.nil?
      self
    end

    def initialize_copy(other)
      return self if equal?(other)
      raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self) if frozen?
      # The holder came over with the instance variables; it is the
      # original's, so let go of it before anything can fail.
      @__ctx__ = nil
      source = other.instance_variable_get(:@__ctx__)
      raise RuntimeError, "Digest CTX wasn't initialized!" if source.nil?
      id = __openssl_copy__(source[0], 1)
      raise OpenSSL.__send__(:__error__, id) if Array === id
      @__ctx__ = OpenSSL.__send__(:__holder__, id)
      self
    end
    private :initialize_copy

    # :call-seq:
    #    digest.reset -> self
    #
    # Resets the Digest in the sense that any Digest#update that has been
    # performed is abandoned and the Digest is set to its initial state again.
    def reset
      ok = __openssl_reset__(__ctx_id__, 1)
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      self
    end

    # :call-seq:
    #    digest.update(string) -> aString
    #
    # Not every message digest can be computed in one single pass. If a
    # message digest is to be computed from several subsequent sources, then
    # each may be passed individually to the Digest instance.
    def update(data)
      __update__(IO.__str__(data))
    end
    alias << update

    # :call-seq:
    #    digest.digest_length -> integer
    #
    # Returns the output size of the digest, i.e. the length in bytes of the
    # final message digest result.
    def digest_length = __info__[1]

    # :call-seq:
    #    digest.block_length -> integer
    #
    # Returns the block length of the digest algorithm, i.e. the length in
    # bytes of an individual block.
    def block_length = __info__[2]

    # :call-seq:
    #    digest.name -> string
    #
    # Returns the short name of this Digest algorithm.
    def name = __info__[0]

    private

    def finish
      value = __openssl_final__(__ctx_id__, 1)
      raise OpenSSL.__send__(:__error__, value) if Array === value
      value
    end

    def __ctx_id__
      holder = @__ctx__
      raise RuntimeError, "Digest CTX wasn't initialized!" if holder.nil?
      holder[0]
    end

    def __update__(data)
      ok = __openssl_update__(__ctx_id__, 1, data)
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      self
    end

    def __info__
      info = __openssl_digest_info__(__ctx_id__)
      raise OpenSSL.__send__(:__error__, info) if Array === info && Integer === info[0]
      info
    end
  end

  # ossl_hmac.c
  class HMACError < OpenSSLError; end

  class HMAC
    # Made in C on CRuby, so `Marshal.dump` refuses one.
    Marshal.__opaque_class__(self)

    # :call-seq:
    #    HMAC.new(key, digest) -> hmac
    #
    # Returns an instance of OpenSSL::HMAC set with the key and digest
    # algorithm to be used. The instance represents the initial state of
    # the message authentication code before any data has been processed.
    def initialize(key, digest)
      id = __ctx_id__
      key = IO.__str__(key)
      algorithm = OpenSSL.__send__(:__algorithm__, digest)
      ok = __openssl_hmac_init__(id, key, algorithm)
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      self
    end

    def initialize_copy(other)
      return self if equal?(other)
      raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self) if frozen?
      # As Digest's: the holder that came over is the original's.
      @__ctx__ = nil
      id = __openssl_copy__(other.__send__(:__ctx_id__), 2)
      raise OpenSSL.__send__(:__error__, id) if Array === id
      @__ctx__ = OpenSSL.__send__(:__holder__, id)
      self
    end
    private :initialize_copy

    # :call-seq:
    #    hmac.reset -> self
    #
    # Returns _hmac_ as it was when it was first initialized, with all
    # processed data cleared from it.
    def reset
      ok = __openssl_reset__(__ctx_id__, 2)
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      self
    end

    # :call-seq:
    #    hmac.update(string) -> self
    #
    # Returns _hmac_ updated with the message to be authenticated. Can be
    # called repeatedly with chunks of the message.
    def update(data)
      data = IO.__str__(data)
      ok = __openssl_update__(__ctx_id__, 2, data)
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      self
    end
    alias << update

    # :call-seq:
    #    hmac.digest -> string
    #
    # Returns the authentication code an instance represents as a binary
    # string.
    def digest
      value = __openssl_final__(__ctx_id__, 2)
      raise OpenSSL.__send__(:__error__, value) if Array === value
      value
    end

    # :call-seq:
    #    hmac.hexdigest -> string
    #
    # Returns the authentication code an instance represents as a
    # hex-encoded string.
    def hexdigest
      value = __openssl_final__(__ctx_id__, 2)
      raise OpenSSL.__send__(:__error__, value) if Array === value
      value.unpack1("H*").b
    end
    alias inspect hexdigest
    alias to_s hexdigest

    private

    # Every HMAC has a context from the moment it is allocated, as
    # `ossl_hmac_alloc` makes one; here it is made the first time it is
    # asked for.
    def __ctx_id__
      holder = @__ctx__
      return holder[0] unless holder.nil?
      id = __openssl_alloc__(2)
      raise OpenSSL.__send__(:__error__, id) if Array === id
      @__ctx__ = OpenSSL.__send__(:__holder__, id)
      id
    end
  end

  # ossl_rand.c
  module Random
    class RandomError < OpenSSLError; end

    module_function

    # :call-seq:
    #   seed(str) -> str
    #
    # ::seed is equivalent to ::add where _entropy_ is length of _str_.
    def seed(str)
      str = IO.__str__(str)
      __openssl_random_seed__(str)
      str
    end

    # :call-seq:
    #   random_add(str, entropy) -> self
    #
    # Mixes the bytes from _str_ into the Pseudo Random Number Generator(PRNG)
    # state.
    def random_add(str, entropy)
      str = IO.__str__(str)
      __openssl_random_add__(str, OpenSSL.__send__(:__double__, entropy))
      self
    end

    # :call-seq:
    #   load_random_file(filename) -> true
    #
    # Reads bytes from _filename_ and adds them to the PRNG.
    def load_random_file(filename)
      ok = __openssl_random_load__(OpenSSL.__send__(:__cstr__, filename))
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      true
    end

    # :call-seq:
    #   write_random_file(filename) -> true
    #
    # Writes a number of random generated bytes (currently 1024) to
    # _filename_ which can be used to initialize the PRNG by calling
    # ::load_random_file in a later session.
    def write_random_file(filename)
      ok = __openssl_random_write__(OpenSSL.__send__(:__cstr__, filename))
      raise OpenSSL.__send__(:__error__, ok) if Array === ok
      true
    end

    # :call-seq:
    #   random_bytes(length) -> string
    #
    # Generates a String with _length_ number of cryptographically strong
    # pseudo-random bytes.
    def random_bytes(len)
      value = __openssl_random_bytes__(OpenSSL.__send__(:__int__, len))
      raise OpenSSL.__send__(:__error__, value) if Array === value
      value
    end

    # :call-seq:
    #   status? => true | false
    #
    # Return +true+ if the PRNG has been seeded with enough data, +false+
    # otherwise.
    def status? = __openssl_random_status__

    class << self
      alias pseudo_bytes random_bytes
    end
  end
end
