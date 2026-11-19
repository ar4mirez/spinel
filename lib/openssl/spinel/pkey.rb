# frozen_string_literal: true

# `ossl_pkey.c` and `ossl_pkey_rsa.c` on Spinel (#107): `OpenSSL::PKey`, an
# `EVP_PKEY` in the heap's registry under an id the object's holder keeps
# (see `openssl/spinel.rb`). Generation, reading, writing, signing and
# verifying are OpenSSL's, through `__openssl_tls__`; `lib/openssl/pkey.rb`
# then adds what upstream writes in Ruby (`RSA.new(size)`, `#public_key`).
#
# A key's class is the one `ossl_pkey_new` picks by its base type: RSA,
# DSA, DH, EC, or PKey::PKey for any other.
#
# ponytail: what needs a key type's own calls is not here: `encrypt`,
# `decrypt`, `derive`, `sign_raw` and its kin, raw keys, `RSA#set_key` and
# the `private_*crypt` family, DSA's, DH's and EC's own methods (groups,
# points, `dh_compute_key`), and passphrase-encrypted export (which needs
# `OpenSSL::Cipher`, a later phase). Upgrade: each one's `EVP_PKEY_*` call
# behind `__openssl_tls__`.

module OpenSSL
  module PKey
    class PKeyError < OpenSSLError; end

    # The result of a primitive: raised when it failed.
    def self.__tls__(*args)
      r = __openssl_tls__(*args)
      raise OpenSSL.__send__(:__error__, r[1]) if Array === r && r[0] == :__failure__
      r
    end

    # A key object for `[id, base id]`, of the class its type has.
    def self.__wrap__(pair)
      id, base = pair
      klass =
        case base
        when 6 then RSA
        when 116 then DSA
        when 28 then DH
        when 408 then EC
        else PKey
        end
      key = klass.allocate
      key.instance_variable_set(:@__pkey__, OpenSSL.__send__(:__holder__, id))
      key
    end

    # `ossl_to_der_if_possible`, and an IO read whole.
    def self.__data__(data)
      data = data.read if data.respond_to?(:read) && !(String === data)
      data = data.to_der if data.respond_to?(:to_der)
      IO.__str__(data)
    end

    def self.__pass__(pass)
      return nil if pass.nil?
      pass = IO.__str__(pass)
      raise PKeyError, "password must not be longer than 1024 bytes" if pass.bytesize > 1024
      pass
    end

    # `ossl_evp_md_fetch`'s argument as a name: a String, or an
    # OpenSSL::Digest's algorithm; nil for none.
    def self.__md_name__(digest)
      return nil if digest.nil?
      return digest.name if OpenSSL::Digest === digest
      OpenSSL.__send__(:__cstr__, digest)
    end

    def self.__options__(options)
      return [] if options.nil?
      Hash.try_convert(options).map { |k, v| [Symbol === k ? k.to_s : String(k), String(v)] }
    end

    private_class_method :__tls__, :__wrap__, :__data__, :__pass__, :__md_name__, :__options__

    # :call-seq:
    #    OpenSSL::PKey.read(string [, pwd ]) -> PKey
    #    OpenSSL::PKey.read(io [, pwd ]) -> PKey
    #
    # Reads a DER or PEM encoded string from _string_ or _io_ and returns an
    # instance of the appropriate PKey class.
    def self.read(data, pass = nil)
      data = __data__(data)
      __wrap__(__tls__("pkey_read", data, __pass__(pass)))
    end

    # :call-seq:
    #    OpenSSL::PKey.generate_key(algo_name [, options]) -> pkey
    #    OpenSSL::PKey.generate_key(pkey [, options]) -> pkey
    #
    # Generates a new key (pair).
    def self.generate_key(algorithm, options = nil)
      __wrap__(__tls__("pkey_generate", __algorithm__(algorithm), __options__(options), false))
    end

    # :call-seq:
    #    OpenSSL::PKey.generate_parameters(algo_name [, options]) -> pkey
    #
    # Generates new parameters for the algorithm.
    def self.generate_parameters(algorithm, options = nil)
      __wrap__(__tls__("pkey_generate", __algorithm__(algorithm), __options__(options), true))
    end

    def self.__algorithm__(algorithm)
      return algorithm.__send__(:__pkey_id__) if PKey === algorithm
      OpenSSL.__send__(:__cstr__, algorithm)
    end
    private_class_method :__algorithm__

    class PKey
      # Made in C on CRuby, so `Marshal.dump` refuses one.
      ::Marshal.__opaque_class__(self)

      def initialize(*)
        raise TypeError, "OpenSSL::PKey::PKey can't be instantiated directly" if instance_of?(PKey)
        super()
      end

      def inspect
        Object.instance_method(:to_s).bind_call(self).sub(/>\z/, " oid=#{oid}>")
      end

      # :call-seq:
      #    pkey.oid -> string
      #
      # Returns the short name of the OID associated with _pkey_.
      def oid = __info__[1]

      # Whether this key has its private half.
      def private? = __info__[2]
      private :private?

      def private_to_der = __export__(1, true)
      def private_to_pem = __export__(1, false)
      def public_to_der = __export__(2, true)
      def public_to_pem = __export__(2, false)

      # :call-seq:
      #    pkey.to_text -> string
      def to_text = __pkey__("pkey_text", __pkey_id__, false)

      # :call-seq:
      #    pkey.sign(digest, data [, options]) -> string
      def sign(digest, data, options = nil)
        __need_private__
        __pkey__("pkey_sign", __pkey_id__, OpenSSL::PKey.__send__(:__md_name__, digest),
          IO.__str__(data), OpenSSL::PKey.__send__(:__options__, options))
      end

      # :call-seq:
      #    pkey.verify(digest, signature, data [, options]) -> true or false
      def verify(digest, signature, data, options = nil)
        __pkey__("pkey_verify", __pkey_id__, OpenSSL::PKey.__send__(:__md_name__, digest),
          IO.__str__(signature), IO.__str__(data), OpenSSL::PKey.__send__(:__options__, options))
      end

      # :call-seq:
      #    pkey.compare?(another_pkey) -> true | false
      def compare?(other)
        raise TypeError, "wrong argument type #{other.class} (expected OpenSSL/EVP_PKEY)" unless PKey === other
        __pkey__("pkey_compare", __pkey_id__, other.__send__(:__pkey_id__))
      end

      private

      def __pkey_id__
        holder = @__pkey__
        raise PKeyError, "no key" if holder.nil?
        holder[0]
      end

      def __pkey__(*args) = OpenSSL::PKey.__send__(:__tls__, *args)

      def __info__ = __pkey__("pkey_info", __pkey_id__)

      def __export__(form, der) = __pkey__("pkey_export", __pkey_id__, form, der)

      # `GetPrivPKeyPtr`'s check.
      def __need_private__
        raise ArgumentError, "private key is needed" unless private?
      end

      def __bn__(name)
        hex = __pkey__("pkey_bn", __pkey_id__, name)
        hex.nil? ? nil : OpenSSL::BN.new(hex, 16)
      end
    end

    class RSA < PKey
      # :call-seq:
      #    RSA.new -> rsa
      #    RSA.new(encoded_key [, password ]) -> rsa
      #    RSA.new(encoded_key) { password } -> rsa
      #    RSA.new(size [, exponent]) -> rsa
      #
      # Generates or loads an \RSA keypair. (`lib/openssl/pkey.rb` handles
      # the size forms.)
      def initialize(*args)
        if args.empty?
          raise ArgumentError, "OpenSSL::PKey::RSA.new cannot be called without arguments; " \
            "pkeys are immutable with OpenSSL 3.0"
        end
        unless args.size <= 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
        end
        data, pass = args
        data = OpenSSL::PKey.__send__(:__data__, data)
        pair = __pkey__("rsa_read", data, OpenSSL::PKey.__send__(:__pass__, pass))
        @__pkey__ = OpenSSL.__send__(:__holder__, pair[0])
      end

      def public? = true
      def private? = super

      def n = __bn__("n")
      def e = __bn__("e")
      def d = __bn__("d")
      def p = __bn__("rsa-factor1")
      def q = __bn__("rsa-factor2")
      def dmp1 = __bn__("rsa-exponent1")
      def dmq1 = __bn__("rsa-exponent2")
      def iqmp = __bn__("rsa-coefficient1")

      # :call-seq:
      #    rsa.export -> PEM-format String
      #
      # Serializes a private or public key to a PEM-encoding: the
      # traditional RSAPrivateKey form for a private key, and
      # SubjectPublicKeyInfo for a public one.
      def export(cipher = nil, pass = nil)
        unless cipher.nil?
          raise NotImplementedError, "encrypted PEM export needs OpenSSL::Cipher, which Spinel does not have yet"
        end
        __export__(private? ? 0 : 2, false)
      end
      alias to_pem export
      alias to_s export

      def to_der = __export__(private? ? 0 : 2, true)

      def sign_pss(digest, data, salt_length:, mgf1_hash:)
        __need_private__
        salt = salt_length == :digest ? "digest" : salt_length == :max ? "max" : Integer(salt_length).to_s
        sign(digest, data, {
          "rsa_padding_mode" => "pss",
          "rsa_pss_saltlen" => salt,
          "rsa_mgf1_md" => OpenSSL::PKey.__send__(:__md_name__, mgf1_hash),
        })
      end

      def verify_pss(digest, signature, data, salt_length:, mgf1_hash:)
        salt = salt_length == :auto ? "auto" : salt_length == :digest ? "digest" : Integer(salt_length).to_s
        verify(digest, signature, data, {
          "rsa_padding_mode" => "pss",
          "rsa_pss_saltlen" => salt,
          "rsa_mgf1_md" => OpenSSL::PKey.__send__(:__md_name__, mgf1_hash),
        })
      end
    end

    # DSA, DH and EC keys read, write, sign and verify as any key does;
    # their own methods are a later slice's (see the top of this file).
    class DSA < PKey
      def initialize(data, pass = nil)
        data = OpenSSL::PKey.__send__(:__data__, data)
        pair = __pkey__("pkey_read", data, OpenSSL::PKey.__send__(:__pass__, pass))
        raise PKeyError, "incorrect pkey type: #{pair[1]}" unless pair[1] == 116
        @__pkey__ = OpenSSL.__send__(:__holder__, pair[0])
      end

      def private? = super
      def public? = true
      def export(cipher = nil, pass = nil) = __export__(private? ? 0 : 2, false)
      alias to_pem export
      alias to_s export
      def to_der = __export__(private? ? 0 : 2, true)
    end

    class DH < PKey
      def private? = super
      def public? = true
    end

    class EC < PKey
      # :call-seq:
      #    EC.generate(ec_group) -> ec
      #
      # Creates a new EC instance with a new random private and public key,
      # on the named curve.
      def self.generate(curve)
        OpenSSL::PKey.generate_key("EC", { "ec_paramgen_curve" => curve, "ec_param_enc" => "named_curve" })
      end

      def initialize(data, pass = nil)
        data = OpenSSL::PKey.__send__(:__data__, data)
        pair = __pkey__("pkey_read", data, OpenSSL::PKey.__send__(:__pass__, pass))
        raise PKeyError, "incorrect pkey type: #{pair[1]}" unless pair[1] == 408
        @__pkey__ = OpenSSL.__send__(:__holder__, pair[0])
      end

      def private? = super
      alias private_key? private?
      def public? = true
      alias public_key? public?
      def export(cipher = nil, pass = nil) = __export__(private? ? 0 : 2, false)
      alias to_pem export
      def to_der = __export__(private? ? 0 : 2, true)
    end
  end
end
