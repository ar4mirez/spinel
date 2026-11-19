# frozen_string_literal: true

# `ossl_x509*.c` on Spinel (#107): `OpenSSL::X509`'s names, extensions,
# certificates, stores and store contexts, over `__openssl_tls__`.
#
# A Certificate, Store or StoreContext is an OpenSSL object in the heap's
# registry under an id its holder keeps (see `openssl/spinel.rb`). A Name or
# an Extension is its DER, which each call decodes: OpenSSL treats both as
# values, and CRuby copies them wherever one is handed over.
#
# A verify callback runs in Ruby, between verifications: the first time
# OpenSSL's callback needs Ruby's answer, verification stops and hands over
# the question (`[preverify_ok, error, depth, cert, chain]`); the answer is
# recorded and the chain verified again, every answer replayed, until no
# question is left. The callback is given a StoreContext that answers what
# the live one would have: the error, its depth, the certificate and the
# chain, and that takes an `error=`.
#
# ponytail: CRLs, attributes and requests (`CRL`, `Revoked`, `Request`,
# `Attribute`, `Store#add_crl`, `StoreContext#current_crl`),
# `Certificate#tbs_bytes` and `ExtensionFactory#config` are not here; the
# Ruby half reopens those classes, which then have none of the C half's
# methods. Upgrade: their `X509_CRL`, `X509_REQ` and `X509_ATTRIBUTE` calls
# behind `__openssl_tls__`.

module OpenSSL
  # The answer to `__openssl_tls__(*args)`, raised when it failed.
  def self.__tls__(*args)
    r = __openssl_tls__(*args)
    raise __error__(r[1]) if Array === r && r[0] == :__failure__
    r
  end

  # `GetPrivPKeyPtr`: a key that answers `private?` must be private.
  def self.__private_key__(key)
    __pkey_of__(key)
    if key.respond_to?(:private?) && !key.private?
      raise ArgumentError, "private key is needed"
    end
    key.__send__(:__pkey_id__)
  end

  def self.__pkey_of__(key)
    unless OpenSSL::PKey::PKey === key
      raise TypeError, "wrong argument type #{key.class} (expected OpenSSL/EVP_PKEY)"
    end
    key.__send__(:__pkey_id__)
  end

  private_class_method :__tls__, :__private_key__, :__pkey_of__

  module X509
    class CertificateError < OpenSSLError; end
    class NameError < OpenSSLError; end
    class ExtensionError < OpenSSLError; end
    class StoreError < OpenSSLError; end
    class AttributeError < OpenSSLError; end
    class CRLError < OpenSSLError; end
    class RequestError < OpenSSLError; end
    class RevokedError < OpenSSLError; end

    # `X509_V_*`, `X509_PURPOSE_*` and `X509_TRUST_*`: OpenSSL 3's
    # `x509_vfy.h` and `x509v3.h` numbers, which are the same in every
    # OpenSSL 3 release.
    {
      V_OK: 0, V_ERR_UNSPECIFIED: 1, V_ERR_UNABLE_TO_GET_ISSUER_CERT: 2,
      V_ERR_UNABLE_TO_GET_CRL: 3, V_ERR_UNABLE_TO_DECRYPT_CERT_SIGNATURE: 4,
      V_ERR_UNABLE_TO_DECRYPT_CRL_SIGNATURE: 5, V_ERR_UNABLE_TO_DECODE_ISSUER_PUBLIC_KEY: 6,
      V_ERR_CERT_SIGNATURE_FAILURE: 7, V_ERR_CRL_SIGNATURE_FAILURE: 8,
      V_ERR_CERT_NOT_YET_VALID: 9, V_ERR_CERT_HAS_EXPIRED: 10, V_ERR_CRL_NOT_YET_VALID: 11,
      V_ERR_CRL_HAS_EXPIRED: 12, V_ERR_ERROR_IN_CERT_NOT_BEFORE_FIELD: 13,
      V_ERR_ERROR_IN_CERT_NOT_AFTER_FIELD: 14, V_ERR_ERROR_IN_CRL_LAST_UPDATE_FIELD: 15,
      V_ERR_ERROR_IN_CRL_NEXT_UPDATE_FIELD: 16, V_ERR_OUT_OF_MEM: 17,
      V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT: 18, V_ERR_SELF_SIGNED_CERT_IN_CHAIN: 19,
      V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY: 20, V_ERR_UNABLE_TO_VERIFY_LEAF_SIGNATURE: 21,
      V_ERR_CERT_CHAIN_TOO_LONG: 22, V_ERR_CERT_REVOKED: 23, V_ERR_PATH_LENGTH_EXCEEDED: 25,
      V_ERR_INVALID_PURPOSE: 26, V_ERR_CERT_UNTRUSTED: 27, V_ERR_CERT_REJECTED: 28,
      V_ERR_SUBJECT_ISSUER_MISMATCH: 29, V_ERR_AKID_SKID_MISMATCH: 30,
      V_ERR_AKID_ISSUER_SERIAL_MISMATCH: 31, V_ERR_KEYUSAGE_NO_CERTSIGN: 32,
      V_ERR_UNABLE_TO_GET_CRL_ISSUER: 33, V_ERR_UNHANDLED_CRITICAL_EXTENSION: 34,
      V_ERR_KEYUSAGE_NO_CRL_SIGN: 35, V_ERR_UNHANDLED_CRITICAL_CRL_EXTENSION: 36,
      V_ERR_INVALID_NON_CA: 37, V_ERR_PROXY_PATH_LENGTH_EXCEEDED: 38,
      V_ERR_KEYUSAGE_NO_DIGITAL_SIGNATURE: 39, V_ERR_PROXY_CERTIFICATES_NOT_ALLOWED: 40,
      V_ERR_INVALID_EXTENSION: 41, V_ERR_INVALID_POLICY_EXTENSION: 42,
      V_ERR_NO_EXPLICIT_POLICY: 43, V_ERR_DIFFERENT_CRL_SCOPE: 44,
      V_ERR_UNSUPPORTED_EXTENSION_FEATURE: 45, V_ERR_UNNESTED_RESOURCE: 46,
      V_ERR_PERMITTED_VIOLATION: 47, V_ERR_EXCLUDED_VIOLATION: 48, V_ERR_SUBTREE_MINMAX: 49,
      V_ERR_APPLICATION_VERIFICATION: 50, V_ERR_UNSUPPORTED_CONSTRAINT_TYPE: 51,
      V_ERR_UNSUPPORTED_CONSTRAINT_SYNTAX: 52, V_ERR_UNSUPPORTED_NAME_SYNTAX: 53,
      V_ERR_CRL_PATH_VALIDATION_ERROR: 54, V_ERR_PATH_LOOP: 55,
      V_ERR_SUITE_B_INVALID_VERSION: 56, V_ERR_SUITE_B_INVALID_ALGORITHM: 57,
      V_ERR_SUITE_B_INVALID_CURVE: 58, V_ERR_SUITE_B_INVALID_SIGNATURE_ALGORITHM: 59,
      V_ERR_SUITE_B_LOS_NOT_ALLOWED: 60, V_ERR_SUITE_B_CANNOT_SIGN_P_384_WITH_P_256: 61,
      V_ERR_HOSTNAME_MISMATCH: 62, V_ERR_EMAIL_MISMATCH: 63, V_ERR_IP_ADDRESS_MISMATCH: 64,
      V_ERR_DANE_NO_MATCH: 65, V_ERR_EE_KEY_TOO_SMALL: 66, V_ERR_CA_KEY_TOO_SMALL: 67,
      V_ERR_CA_MD_TOO_WEAK: 68, V_ERR_INVALID_CALL: 69, V_ERR_STORE_LOOKUP: 70,
      V_ERR_NO_VALID_SCTS: 71, V_ERR_PROXY_SUBJECT_NAME_VIOLATION: 72,
      V_ERR_OCSP_VERIFY_NEEDED: 73, V_ERR_OCSP_VERIFY_FAILED: 74, V_ERR_OCSP_CERT_UNKNOWN: 75,
      V_ERR_INVALID_CA: 79,
      V_FLAG_USE_CHECK_TIME: 0x2, V_FLAG_CRL_CHECK: 0x4, V_FLAG_CRL_CHECK_ALL: 0x8,
      V_FLAG_IGNORE_CRITICAL: 0x10, V_FLAG_X509_STRICT: 0x20, V_FLAG_ALLOW_PROXY_CERTS: 0x40,
      V_FLAG_POLICY_CHECK: 0x80, V_FLAG_EXPLICIT_POLICY: 0x100, V_FLAG_INHIBIT_ANY: 0x200,
      V_FLAG_INHIBIT_MAP: 0x400, V_FLAG_NOTIFY_POLICY: 0x800,
      V_FLAG_EXTENDED_CRL_SUPPORT: 0x1000, V_FLAG_USE_DELTAS: 0x2000,
      V_FLAG_CHECK_SS_SIGNATURE: 0x4000, V_FLAG_TRUSTED_FIRST: 0x8000,
      V_FLAG_SUITEB_128_LOS_ONLY: 0x10000, V_FLAG_SUITEB_192_LOS: 0x20000,
      V_FLAG_SUITEB_128_LOS: 0x30000, V_FLAG_PARTIAL_CHAIN: 0x80000,
      V_FLAG_NO_ALT_CHAINS: 0x100000, V_FLAG_NO_CHECK_TIME: 0x200000,
      PURPOSE_SSL_CLIENT: 1, PURPOSE_SSL_SERVER: 2, PURPOSE_NS_SSL_SERVER: 3,
      PURPOSE_SMIME_SIGN: 4, PURPOSE_SMIME_ENCRYPT: 5, PURPOSE_CRL_SIGN: 6, PURPOSE_ANY: 7,
      PURPOSE_OCSP_HELPER: 8, PURPOSE_TIMESTAMP_SIGN: 9,
      TRUST_COMPAT: 1, TRUST_SSL_CLIENT: 2, TRUST_SSL_SERVER: 3, TRUST_EMAIL: 4,
      TRUST_OBJECT_SIGN: 5, TRUST_OCSP_SIGN: 6, TRUST_OCSP_REQUEST: 7, TRUST_TSA: 8,
    }.each { |name, value| const_set(name, value) }

    defaults = OpenSSL.__send__(:__tls__, "defaults")
    DEFAULT_CERT_FILE = defaults[0].freeze
    DEFAULT_CERT_DIR = defaults[1].freeze
    DEFAULT_CERT_FILE_ENV = defaults[2].freeze
    DEFAULT_CERT_DIR_ENV = defaults[3].freeze
    DEFAULT_CERT_AREA = defaults[4].freeze
    DEFAULT_PRIVATE_DIR = defaults[5].freeze

    # A certificate object for registry id `id`.
    def self.__cert__(id)
      cert = Certificate.allocate
      cert.instance_variable_set(:@__cert__, OpenSSL.__send__(:__holder__, id))
      cert
    end

    def self.__cert_id__(cert)
      unless Certificate === cert
        raise TypeError, "wrong argument type #{cert.class} (expected OpenSSL/X509)"
      end
      cert.__send__(:__cert_id__)
    end

    def self.__name_der__(name)
      raise TypeError, "wrong argument type #{name.class} (expected OpenSSL/X509/NAME)" unless Name === name
      name.to_der
    end

    def self.__ext_der__(ext)
      raise TypeError, "wrong argument type #{ext.class} (expected OpenSSL/X509/EXTENSION)" unless Extension === ext
      ext.to_der
    end

    # `ossl_to_der_if_possible`.
    def self.__der__(value)
      value = value.to_der if value.respond_to?(:to_der)
      IO.__str__(value)
    end

    private_class_method :__cert__, :__cert_id__, :__name_der__, :__ext_der__, :__der__

    class Name
      include Comparable

      COMPAT = 0
      RFC2253 = 17_892_119
      ONELINE = 8_520_479
      MULTILINE = 44_302_342
      DEFAULT_OBJECT_TYPE = OpenSSL::ASN1::UTF8STRING
      OBJECT_TYPE_TEMPLATE = Hash.new(OpenSSL::ASN1::UTF8STRING).tap { |h|
        %w[C countryName serialNumber dnQualifier].each { |k| h[k] = OpenSSL::ASN1::PRINTABLESTRING }
        %w[DC domainComponent emailAddress].each { |k| h[k] = OpenSSL::ASN1::IA5STRING }
      }.freeze

      def self.__from_der__(der)
        name = allocate
        name.instance_variable_set(:@__der__, der)
        name
      end

      # :call-seq:
      #    X509::Name.new                               => name
      #    X509::Name.new(der)                          => name
      #    X509::Name.new(distinguished_name)           => name
      #    X509::Name.new(distinguished_name, template) => name
      def initialize(*args)
        unless args.size <= 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..2)"
        end
        @__der__ = OpenSSL.__send__(:__tls__, "name_new")
        return if args.empty?
        arg, template = args
        entries = Array.try_convert(arg)
        if entries
          template = OBJECT_TYPE_TEMPLATE if template.nil?
          entries.each do |entry|
            raise TypeError, "wrong argument type #{entry.class} (expected Array)" unless Array === entry
            type = entry[2]
            type = template[entry[0]] if type.nil?
            type = DEFAULT_OBJECT_TYPE if type.nil?
            add_entry(entry[0], entry[1], type)
          end
        else
          der = X509.__send__(:__der__, arg).b
          OpenSSL.__send__(:__tls__, "name_check", der)
          @__der__ = der
        end
      end

      def initialize_copy(other)
        super
        @__der__ = other.to_der
        self
      end

      # :call-seq:
      #    name.add_entry(oid, value [, type], loc: -1, set: 0) => self
      def add_entry(oid, value, type = nil, loc: -1, set: 0)
        oid = OpenSSL.__send__(:__cstr__, oid)
        value = IO.__str__(value)
        type = OBJECT_TYPE_TEMPLATE[oid] if type.nil?
        @__der__ = OpenSSL.__send__(:__tls__, "name_add", @__der__, oid, value,
          OpenSSL.__send__(:__int__, type), OpenSSL.__send__(:__int__, loc), OpenSSL.__send__(:__int__, set))
        self
      end

      # :call-seq:
      #    name.to_s         -> string
      #    name.to_s(format) -> string
      def to_s(*args)
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
        format = args[0]
        format = Integer.__index__(format) unless format.nil?
        OpenSSL.__send__(:__tls__, "name_to_s", @__der__, format)
      end

      # :call-seq:
      #    name.to_utf8 -> string
      def to_utf8
        flags = RFC2253 & ~0x4 # ASN1_STRFLGS_ESC_MSB
        OpenSSL.__send__(:__tls__, "name_to_s", @__der__, flags).force_encoding(Encoding::UTF_8)
      end

      def inspect = "#<#{self.class} #{to_utf8}>"

      # :call-seq:
      #    name.to_a => [[name, data, type], ...]
      def to_a = OpenSSL.__send__(:__tls__, "name_to_a", @__der__)

      # :call-seq:
      #    name.cmp(other) -> -1 | 0 | 1 | nil
      def cmp(other)
        return nil unless Name === other
        OpenSSL.__send__(:__tls__, "name_cmp", @__der__, other.to_der) <=> 0
      end
      alias <=> cmp

      def eql?(other) = Name === other && cmp(other) == 0

      def hash = OpenSSL.__send__(:__tls__, "name_hash", @__der__, false)
      def hash_old = OpenSSL.__send__(:__tls__, "name_hash", @__der__, true)

      def to_der = @__der__.dup
    end

    class Extension
      # :call-seq:
      #    OpenSSL::X509::Extension.new(der)
      #    OpenSSL::X509::Extension.new(oid, value)
      #    OpenSSL::X509::Extension.new(oid, value, critical)
      def initialize(*args)
        unless (1..3).cover?(args.size)
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..3)"
        end
        if args.size == 1
          der = X509.__send__(:__der__, args[0]).b
          OpenSSL.__send__(:__tls__, "ext_info", der)
          @__der__ = der
        else
          oid, value, critical = args
          @__der__ = __build__(OpenSSL.__send__(:__cstr__, oid), X509.__send__(:__der__, value), critical ? true : false)
        end
      end

      def initialize_copy(other)
        super
        @__der__ = other.to_der
        self
      end

      def oid = __info__[0]
      def value = __info__[1]
      def critical? = __info__[2]
      def value_der = __info__[3]

      def oid=(oid)
        _, _, critical, der = __info__
        @__der__ = __build__(OpenSSL.__send__(:__cstr__, oid), der, critical)
        oid
      end

      def value=(data)
        oid, _, critical, = __info__
        @__der__ = __build__(oid, X509.__send__(:__der__, data), critical)
        data
      end

      def critical=(flag)
        oid, _, _, der = __info__
        @__der__ = __build__(oid, der, flag ? true : false)
        flag
      end

      def to_der = @__der__.dup

      private

      def __info__ = OpenSSL.__send__(:__tls__, "ext_info", @__der__)

      def __build__(oid, der, critical) = OpenSSL.__send__(:__tls__, "ext_new", oid, der, critical)
    end

    class ExtensionFactory
      attr_reader :issuer_certificate, :subject_certificate, :subject_request, :crl, :config

      def initialize(issuer_cert = nil, subject_cert = nil, subject_req = nil, crl = nil)
        @config = nil
        self.issuer_certificate = issuer_cert unless issuer_cert.nil?
        self.subject_certificate = subject_cert unless subject_cert.nil?
        self.subject_request = subject_req unless subject_req.nil?
        self.crl = crl unless crl.nil?
      end

      def issuer_certificate=(cert)
        X509.__send__(:__cert_id__, cert)
        @issuer_certificate = cert
      end

      def subject_certificate=(cert)
        X509.__send__(:__cert_id__, cert)
        @subject_certificate = cert
      end

      def subject_request=(req)
        raise NotImplementedError, "OpenSSL::X509::Request is not supported on Spinel yet"
      end

      def crl=(crl)
        raise NotImplementedError, "OpenSSL::X509::CRL is not supported on Spinel yet"
      end

      def config=(config)
        raise NotImplementedError, "OpenSSL::Config is not supported on Spinel yet" unless config.nil?
        @config = config
      end

      # :call-seq:
      #    ef.create_ext(ln_or_sn, "value", critical = false) -> X509::Extension
      #    ef.create_ext(ln_or_sn, "critical,value")          -> X509::Extension
      def create_ext(*args)
        unless (2..3).cover?(args.size)
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 2..3)"
        end
        oid, value, critical = args
        oid = OpenSSL.__send__(:__cstr__, oid)
        value = IO.__str__(value)
        valstr = (critical ? "critical," : "") + value
        issuer = @issuer_certificate&.__send__(:__cert_id__)
        subject = @subject_certificate&.__send__(:__cert_id__)
        Extension.__send__(:__from_der__, OpenSSL.__send__(:__tls__, "ext_create", oid, valstr, issuer, subject))
      end
    end

    class Extension
      def self.__from_der__(der)
        ext = allocate
        ext.instance_variable_set(:@__der__, der)
        ext
      end
      private_class_method :__from_der__
    end

    class Certificate
      # :call-seq:
      #    Certificate.new => cert
      #    Certificate.new(string) => cert
      def initialize(*args)
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
        id =
          if args.empty?
            OpenSSL.__send__(:__tls__, "cert_new")
          else
            OpenSSL.__send__(:__tls__, "cert_read", X509.__send__(:__der__, args[0]))
          end
        @__cert__ = OpenSSL.__send__(:__holder__, id)
      end

      def initialize_copy(other)
        super
        @__cert__ = nil
        @__cert__ = OpenSSL.__send__(:__holder__, OpenSSL.__send__(:__tls__, "cert_dup", other.__send__(:__cert_id__)))
        self
      end

      # :call-seq:
      #    OpenSSL::X509::Certificate.load(string) -> [certs...]
      #    OpenSSL::X509::Certificate.load(file) -> [certs...]
      def self.load(data)
        data = data.read if data.respond_to?(:read)
        OpenSSL.__send__(:__tls__, "cert_load", IO.__str__(data)).map { |id| X509.__send__(:__cert__, id) }
      end

      def version = __get__("version")

      def version=(version)
        __set__("version", Integer.__index__(version))
        version
      end

      def serial = OpenSSL::BN.new(__get__("serial"), 16)

      def serial=(num)
        __set__("serial", OpenSSL::BN.__send__(:__int__, num).to_s(16))
        num
      end

      def signature_algorithm = __get__("signature_algorithm")

      def subject = Name.__from_der__(__get__("subject"))

      def subject=(name)
        __set__("subject", X509.__send__(:__name_der__, name))
        name
      end

      def issuer = Name.__from_der__(__get__("issuer"))

      def issuer=(name)
        __set__("issuer", X509.__send__(:__name_der__, name))
        name
      end

      def not_before = Time.utc(*__get__("not_before"))

      def not_before=(time)
        __set__("not_before", Integer(time))
        time
      end

      def not_after = Time.utc(*__get__("not_after"))

      def not_after=(time)
        __set__("not_after", Integer(time))
        time
      end

      def public_key = OpenSSL::PKey.__send__(:__wrap__, __get__("public_key"))

      def public_key=(key)
        __set__("public_key", OpenSSL.__send__(:__pkey_of__, key))
        key
      end

      # :call-seq:
      #    cert.sign(key, digest) => self
      def sign(key, digest)
        key_id = OpenSSL.__send__(:__private_key__, key)
        OpenSSL.__send__(:__tls__, "cert_sign", __cert_id__, key_id, OpenSSL::PKey.__send__(:__md_name__, digest))
        self
      end

      # :call-seq:
      #    cert.verify(key) => true | false
      def verify(key)
        OpenSSL.__send__(:__tls__, "cert_verify", __cert_id__, OpenSSL.__send__(:__pkey_of__, key))
      end

      # :call-seq:
      #    cert.check_private_key(key) -> true | false
      def check_private_key(key)
        OpenSSL.__send__(:__tls__, "cert_check_private_key", __cert_id__, OpenSSL.__send__(:__pkey_of__, key))
      end

      def extensions
        OpenSSL.__send__(:__tls__, "cert_extensions", __cert_id__).map { |der| Extension.__send__(:__from_der__, der) }
      end

      def extensions=(extensions)
        raise TypeError, "wrong argument type #{extensions.class} (expected Array)" unless Array === extensions
        ders = extensions.map { |e| X509.__send__(:__ext_der__, e) }
        OpenSSL.__send__(:__tls__, "cert_clear_extensions", __cert_id__)
        ders.each { |der| OpenSSL.__send__(:__tls__, "cert_add_extension", __cert_id__, der) }
        extensions
      end

      def add_extension(extension)
        OpenSSL.__send__(:__tls__, "cert_add_extension", __cert_id__, X509.__send__(:__ext_der__, extension))
        extension
      end

      def to_der = __get__("der")
      def to_pem = __get__("pem")
      alias to_s to_pem
      def to_text = __get__("text")

      def ==(other)
        return false unless Certificate === other
        OpenSSL.__send__(:__tls__, "cert_eq", __cert_id__, other.__send__(:__cert_id__))
      end

      private

      def __cert_id__
        holder = @__cert__
        raise CertificateError, "no certificate" if holder.nil?
        holder[0]
      end

      def __get__(field) = OpenSSL.__send__(:__tls__, "cert_get", __cert_id__, field)
      def __set__(field, value) = OpenSSL.__send__(:__tls__, "cert_set", __cert_id__, field, value)
    end

    class Store
      attr_reader :verify_callback, :error, :error_string, :chain

      def initialize(*args)
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0)" unless args.empty?
        @__store__ = OpenSSL.__send__(:__holder__, OpenSSL.__send__(:__tls__, "store_new"))
        @verify_callback = nil
        @error = nil
        @error_string = nil
        @chain = nil
      end

      def verify_callback=(callback)
        @verify_callback = callback
      end

      def flags=(flags)
        __op__("flags", Integer.__index__(flags))
        flags
      end

      def purpose=(purpose)
        __op__("purpose", OpenSSL.__send__(:__int__, purpose))
        purpose
      end

      def trust=(trust)
        __op__("trust", OpenSSL.__send__(:__int__, trust))
        trust
      end

      def time=(time)
        __op__("time", Integer(time))
        time
      end

      def add_path(path)
        __op__("add_path", OpenSSL.__send__(:__cstr__, File.path(path)))
        self
      end

      def add_file(file)
        __op__("add_file", OpenSSL.__send__(:__cstr__, File.path(file)))
        self
      end

      def set_default_paths
        __op__("set_default_paths", nil)
        nil
      end

      def add_cert(cert)
        __op__("add_cert", X509.__send__(:__cert_id__, cert))
        self
      end

      def add_crl(crl)
        raise NotImplementedError, "OpenSSL::X509::CRL is not supported on Spinel yet"
      end

      # :call-seq:
      #    store.verify(cert, chain = nil) -> true | false
      def verify(cert, chain = nil, &block)
        ctx = StoreContext.new(self, cert, chain)
        ctx.verify_callback = block || @verify_callback
        result = ctx.verify
        @error = ctx.error
        @error_string = ctx.error_string
        @chain = ctx.chain
        result
      end

      private

      def __store_id__ = @__store__[0]

      def __op__(op, arg) = OpenSSL.__send__(:__tls__, "store_op", __store_id__, op, arg)
    end

    class StoreContext
      attr_accessor :verify_callback

      # :call-seq:
      #    StoreContext.new(store, cert = nil, untrusted = nil)
      def initialize(store, cert = nil, chain = nil)
        raise TypeError, "wrong argument type #{store.class} (expected OpenSSL/X509/STORE)" unless Store === store
        cert_id = cert.nil? ? nil : X509.__send__(:__cert_id__, cert)
        chain_ids = chain.nil? ? nil : Array(chain).map { |c| X509.__send__(:__cert_id__, c) }
        id = OpenSSL.__send__(:__tls__, "stctx_new", store.__send__(:__store_id__), cert_id, chain_ids)
        @__stctx__ = OpenSSL.__send__(:__holder__, id)
        @verify_callback = store.verify_callback
        @cert = cert
      end

      # :call-seq:
      #    stctx.verify -> true | false
      def verify
        callback = @verify_callback
        loop do
          result, question = __tls__("stctx_verify", __stctx_id__, !callback.nil?)
          return result if question.nil?
          ok, error = StoreContext.__answer__(callback, question)
          __tls__("stctx_set", __stctx_id__, "answer", ok, error)
        end
      end

      def error = __snapshot__ ? @__snapshot__[:error] : __tls__("stctx_get", __stctx_id__, "error")

      def error=(error)
        error = OpenSSL.__send__(:__int__, error)
        if __snapshot__
          @__snapshot__[:error] = error
        else
          __tls__("stctx_set", __stctx_id__, "error", error)
        end
        error
      end

      def error_depth = __snapshot__ ? @__snapshot__[:depth] : __tls__("stctx_get", __stctx_id__, "error_depth")

      def error_string = __tls__("verify_error_string", error)

      def chain
        ders = __snapshot__ ? @__snapshot__[:chain] : __tls__("stctx_get", __stctx_id__, "chain")
        ders&.map { |der| Certificate.new(der) }
      end

      def current_cert
        der = __snapshot__ ? @__snapshot__[:cert] : __tls__("stctx_get", __stctx_id__, "current_cert")
        der && Certificate.new(der)
      end

      def current_crl = nil

      def flags=(flags)
        __tls__("stctx_set", __stctx_id__, "flags", Integer.__index__(flags))
        flags
      end

      def purpose=(purpose)
        __tls__("stctx_set", __stctx_id__, "purpose", OpenSSL.__send__(:__int__, purpose))
        purpose
      end

      def trust=(trust)
        __tls__("stctx_set", __stctx_id__, "trust", OpenSSL.__send__(:__int__, trust))
        trust
      end

      def time=(time)
        __tls__("stctx_set", __stctx_id__, "time", Integer(time))
        time
      end

      # The StoreContext a verify callback is given: what OpenSSL's
      # callback was, at the question.
      def self.__snapshot__(question)
        ok, error, depth, cert, chain = question
        ctx = allocate
        ctx.instance_variable_set(:@__snapshot__, { ok: ok, error: error, depth: depth, cert: cert, chain: chain })
        ctx
      end

      # `ossl_verify_cb_call`: `[ok, error]` once `callback` has answered
      # `question`. With no callback, OpenSSL's own verdict.
      def self.__answer__(callback, question, preverify_ok = question[0], error = question[1])
        return [preverify_ok, error] if callback.nil?
        ctx = __snapshot__(question)
        ctx.error = error
        ret = false
        begin
          ret = callback.call(preverify_ok, ctx)
        rescue Exception
          warn "exception in verify_callback is ignored", uplevel: 0
        end
        return [true, OpenSSL::X509::V_OK] if ret == true
        error = ctx.error
        error = OpenSSL::X509::V_ERR_CERT_REJECTED if error == OpenSSL::X509::V_OK
        [false, error]
      end
      private_class_method :__snapshot__

      private

      def __snapshot__ = !@__snapshot__.nil?

      def __stctx_id__
        holder = @__stctx__
        raise StoreError, "StoreContext is not initialized" if holder.nil?
        holder[0]
      end

      def __tls__(*args) = OpenSSL.__send__(:__tls__, *args)
    end
  end
end
