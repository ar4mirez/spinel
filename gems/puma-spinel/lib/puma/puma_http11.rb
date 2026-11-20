# frozen_string_literal: true

# The puma gem's C extension (`ext/puma_http11/puma_http11.c` and
# `mini_ssl.c` at 8.1.2), in Ruby, over `Puma::Spinel`, the gem's `.spinel`
# extension (`ext/puma_spinel`). Upstream's `lib/puma.rb` and
# `lib/puma/minissl.rb` require this file where CRuby loads
# `puma_http11.so`; the rest of `lib/` is upstream's, unchanged.
#
# Each method does what its C function does: the same checks in the same
# order, the same exception classes and messages, and the same answers. The
# HTTP parser's state machine and MiniSSL's OpenSSL calls are the
# extension's; what the C's callbacks do with Ruby objects is here. The C's
# state is in instance variables spelled `@__name__`, which
# `instance_variables` leaves out as it leaves out a typed data struct.

unless RUBY_ENGINE == "spinel"
  raise LoadError, "puma-spinel is the puma gem for Spinel and loads only there; " \
                   "on #{RUBY_ENGINE}, install the puma gem"
end

require "puma/puma_spinel"

module Puma
  module Spinel
    class << self
      # The records the extension packed: `[kind, fields]` pairs, each
      # field a BINARY String.
      def records(packed)
        packed.force_encoding(Encoding::BINARY)
        out = []
        pos = 0
        size = packed.bytesize
        while pos < size
          kind = packed.getbyte(pos)
          count = packed.getbyte(pos + 1)
          pos += 2
          fields = Array.new(count) do
            len = packed.byteslice(pos, 4).unpack1("L<")
            pos += 4
            field = packed.byteslice(pos, len)
            pos += len
            field
          end
          out << [kind, fields]
        end
        out
      end

      # A finalizer freeing the box `free` names, by its handle.
      def finalizer(free, handle)
        proc { Spinel.__send__(free, handle) }
      end

      # `StringValue(value)`: a String, or what `to_str` makes of one.
      def string_value(value)
        return value if String === value
        unless value.respond_to?(:to_str)
          raise TypeError, "no implicit conversion of #{type_name(value)} into String"
        end
        converted = value.to_str
        unless String === converted
          raise TypeError, "can't convert #{value.class} to String (#{value.class}#to_str gives #{converted.class})"
        end
        converted
      end

      # `RSTRING_PTR(value)` with no check, which the C reads as a C string:
      # a String's bytes, anything else a `TypeError` here where the C
      # reads past an object that is not a String.
      def rstring_ptr(value)
        string_value(value)
      end

      # `NUM2INT(value)`.
      def num2int(value)
        int = case value
        when Integer then value
        when Float
          raise RangeError, "float #{value} out of range of integer" unless value.finite?
          value.to_i
        when nil
          raise TypeError, "no implicit conversion from nil to integer"
        when true, false
          raise TypeError, "no implicit conversion of #{value} into Integer"
        else
          unless value.respond_to?(:to_int)
            raise TypeError, "no implicit conversion of #{type_name(value)} into Integer"
          end
          value.to_int
        end
        unless (-2**31...2**31).cover?(int)
          raise RangeError, "integer #{int} too #{int < 0 ? "small" : "big"} to convert to 'int'"
        end
        int
      end

      # How `rb_raise(rb_eTypeError, ...)` names a value's class.
      def type_name(value)
        case value
        when nil then "nil"
        when true then "true"
        when false then "false"
        else value.class.to_s
        end
      end
    end

    # `rb_enc_interned_str_cstr(name, rb_utf8_encoding())`: the frozen,
    # deduplicated UTF-8 String.
    def self.interned(bytes)
      -bytes.dup.force_encoding(Encoding::UTF_8)
    end

    # `DEF_MAX_LENGTH`'s limits as its message prints them: the C
    # stringifies each limit's source text, expanded, not its value.
    MAX_LENGTH_TEXT = {
      "FIELD_NAME" => "256",
      "FIELD_VALUE" => "80 * 1024",
      "REQUEST_URI" => "(1024 * 12)",
      "FRAGMENT" => "1024",
      "REQUEST_PATH" => "(8192)",
      "QUERY_STRING" => "(1024 * 10)",
      "HEADER" => "(1024 * (80 + 32))"
    }.freeze

    REQUEST_METHOD = interned("REQUEST_METHOD")
    REQUEST_URI = interned("REQUEST_URI")
    FRAGMENT = interned("FRAGMENT")
    QUERY_STRING = interned("QUERY_STRING")
    SERVER_PROTOCOL = interned("SERVER_PROTOCOL")
    REQUEST_PATH = interned("REQUEST_PATH")

    # `common_http_fields`: the keys the C keeps one String of, raw for the
    # two CGI names without the `HTTP_` prefix.
    COMMON_FIELDS = %w[
      ACCEPT ACCEPT_CHARSET ACCEPT_ENCODING ACCEPT_LANGUAGE ALLOW
      AUTHORIZATION CACHE_CONTROL CONNECTION CONTENT_ENCODING CONTENT_LENGTH
      CONTENT_TYPE COOKIE DATE EXPECT FROM HOST IF_MATCH IF_MODIFIED_SINCE
      IF_NONE_MATCH IF_RANGE IF_UNMODIFIED_SINCE KEEP_ALIVE MAX_FORWARDS
      PRAGMA PROXY_AUTHORIZATION RANGE REFERER TE TRAILER TRANSFER_ENCODING
      UPGRADE USER_AGENT VIA X_FORWARDED_FOR X_REAL_IP WARNING
    ].to_h do |name|
      raw = name == "CONTENT_LENGTH" || name == "CONTENT_TYPE"
      [name.b.freeze, interned(raw ? name : "HTTP_#{name}")]
    end.freeze
  end

  class HttpParserError < StandardError
  end

  # `Puma.enable_child_subreaper`: marks this process a child subreaper,
  # true, or false where the C has no `prctl`.
  def self.enable_child_subreaper
    answer = Spinel.child_subreaper
    if Integer === answer
      raise SystemCallError.new("prctl(PR_SET_CHILD_SUBREAPER)", -answer)
    end
    answer
  end

  class HttpParser
    # A copy is a new parser, as `rb_obj_dup` allocates one with
    # `HttpParser_alloc`.
    private def initialize_copy(original)
      super
      ObjectSpace.undefine_finalizer(self)
      @__handle__ = nil
      @__body__ = nil
      @__request__ = nil
      self
    end

    # `HttpParser_init`.
    def initialize
      Spinel.parser_init(__handle__)
      @__body__ = nil
      self
    end

    # `HttpParser_reset`.
    def reset
      Spinel.parser_init(__handle__)
      @__body__ = nil
      nil
    end

    # `HttpParser_finish`: `puma_parser_finish` leaves the state as it is.
    def finish
      Spinel.parser_finished?(__handle__)
    end

    # `HttpParser_execute`: parses `data` from `start`, filling in
    # `req_hash` as the C's callbacks do, and answers how much of it has
    # been read; raises `Puma::HttpParserError` when the request is not
    # HTTP or an element is too long.
    def execute(req_hash, data, start)
      from = Integer(start)
      if from >= data.bytesize
        raise HttpParserError, "Requested start is after data buffer end."
      end
      @__request__ = req_hash
      packed = Spinel.parser_execute(__handle__, data, from)
      Spinel.records(packed).each do |kind, fields|
        case kind
        when 0x4d # M
          req_hash[Spinel::REQUEST_METHOD] = fields[0]
        when 0x55 # U
          req_hash[Spinel::REQUEST_URI] = fields[0]
        when 0x46 # F
          req_hash[Spinel::FRAGMENT] = fields[0]
        when 0x50 # P
          req_hash[Spinel::REQUEST_PATH] = fields[0]
        when 0x51 # Q
          req_hash[Spinel::QUERY_STRING] = fields[0]
        when 0x56 # V
          req_hash[Spinel::SERVER_PROTOCOL] = fields[0]
        when 0x48 # H
          http_field(req_hash, fields[0], fields[1])
        when 0x42 # B
          @__body__ = fields[0]
        when 0x58 # X
          upcased(data, Integer(fields[0]), fields[1])
        when 0x45 # E
          too_long(fields[0], Integer(fields[2]))
        end
      end

      nread = Spinel.parser_nread(__handle__)
      too_long("HEADER", nread) if nread > 114_688
      if Spinel.parser_error?(__handle__)
        raise HttpParserError, "Invalid HTTP format, parsing fails. Are you trying to open an SSL connection to a non-SSL Puma?"
      end
      nread
    end

    # `HttpParser_has_error`.
    def error?
      Spinel.parser_error?(__handle__)
    end

    # `HttpParser_is_finished`.
    def finished?
      Spinel.parser_finished?(__handle__)
    end

    # `HttpParser_nread`.
    def nread
      Spinel.parser_nread(__handle__)
    end

    # `HttpParser_body`.
    def body
      @__body__
    end

    private

    # The parser's box, made on first use as `HttpParser_alloc` makes it
    # when the object is: `Class#new` allocates without calling a Ruby
    # `allocate`.
    def __handle__
      @__handle__ ||= begin
        handle = Spinel.parser_new
        ObjectSpace.define_finalizer(self, Spinel.finalizer(:parser_free, handle))
        handle
      end
    end

    # `http_field`: the header under its common key or `HTTP_` and its
    # name, a duplicate joined to the first with a comma (the extension
    # joins those of one `execute`).
    def http_field(req_hash, name, value)
      key = Spinel::COMMON_FIELDS[name] || Spinel.interned("HTTP_".b + name)
      existing = req_hash[key]
      if existing.nil?
        req_hash[key] = value
      else
        existing << ", " << value
      end
    end

    # `snake_upcase_char`'s writes, which the C makes in the String it
    # parses: `bytes` in place from byte `at`.
    #
    # ponytail: the C writes into a frozen String too, which Ruby cannot,
    # so a frozen one is left as it was, and a header name `execute` reads
    # in two calls on it keeps the first call's part as sent. Puma parses
    # its own unfrozen buffer. Upgrade: `spinel-ext` writing a String's
    # bytes.
    def upcased(data, at, bytes)
      return if data.frozen?
      tail = at + bytes.bytesize
      patched = data.byteslice(0, at) << bytes.force_encoding(data.encoding) <<
        data.byteslice(tail, data.bytesize - tail)
      data.replace(patched)
    end

    # `VALIDATE_MAX_LENGTH`'s raise, the length printed with `%d`.
    def too_long(name, len)
      len = [len].pack("q").unpack1("l")
      raise HttpParserError, "HTTP element #{name} is longer than the " \
                             "#{Spinel::MAX_LENGTH_TEXT.fetch(name)} allowed length (was #{len})"
    end
  end

  # `Init_mini_ssl`: MiniSSL's context and engine over `Puma::Spinel`'s
  # OpenSSL calls.
  module MiniSSL
    constants = Spinel.records(Spinel.ssl_constants).first.last

    # Version of OpenSSL that Puma was compiled with
    OPENSSL_VERSION = constants[0]
    # Version of OpenSSL that Puma loaded with
    OPENSSL_LIBRARY_VERSION = constants[1]
    # True if SSL3 is not available
    OPENSSL_NO_SSL3 = constants[2] == "1"
    # True if TLS1 is not available
    OPENSSL_NO_TLS1 = constants[3] == "1"
    # True if TLS1_1 is not available
    OPENSSL_NO_TLS1_1 = constants[4] == "1"

    def self.check
      nil
    end

    class SSLError < StandardError
    end

    class SSLContext
      # `sslctx_alloc`, made on first use: `Class#new` allocates without
      # calling a Ruby `allocate`.
      private def __ctx__
        @__ctx__ ||= begin
          handle = Spinel.ctx_new
          raise SSLError, "SSL_CTX_new" if handle.nil?
          ObjectSpace.define_finalizer(self, Spinel.finalizer(:ctx_free, handle))
          handle
        end
      end

      undef_method :initialize_copy

      # `sslctx_initialize`: the context `mini_ssl_ctx`, a
      # `Puma::MiniSSL::Context`, describes, its settings read in the C's
      # order, then frozen.
      def initialize(mini_ssl_ctx)
        ctx = __ctx__
        reuse = mini_ssl_ctx.reuse
        reuse_cache_size = mini_ssl_ctx.reuse_cache_size
        reuse_timeout = mini_ssl_ctx.reuse_timeout
        key = mini_ssl_ctx.key
        key_password_command = mini_ssl_ctx.key_password_command
        cert = mini_ssl_ctx.cert
        ca = mini_ssl_ctx.ca
        cert_pem = mini_ssl_ctx.cert_pem
        key_pem = mini_ssl_ctx.key_pem
        verify_mode = mini_ssl_ctx.verify_mode
        ssl_cipher_filter = mini_ssl_ctx.ssl_cipher_filter
        ssl_ciphersuites = mini_ssl_ctx.ssl_ciphersuites
        no_tlsv1 = mini_ssl_ctx.no_tlsv1
        no_tlsv1_1 = mini_ssl_ctx.no_tlsv1_1

        unless cert.nil?
          ssl_raise Spinel.ctx_use_certificate_chain_file(ctx, Spinel.string_value(cert))
        end

        password = nil
        unless key_password_command.nil?
          key_password = mini_ssl_ctx.key_password
          password = Spinel.string_value(key_password) unless key_password.nil?
        end

        unless key.nil?
          ssl_raise Spinel.ctx_use_private_key_file(ctx, Spinel.string_value(key), password)
        end

        ssl_raise Spinel.ctx_cert_pem(ctx, Spinel.rstring_ptr(cert_pem)) unless cert_pem.nil?
        ssl_raise Spinel.ctx_key_pem(ctx, Spinel.rstring_ptr(key_pem), password) unless key_pem.nil?

        verification_flags = mini_ssl_ctx.verification_flags
        unless verification_flags.nil?
          Spinel.ctx_verification_flags(ctx, Spinel.num2int(verification_flags))
        end

        ssl_raise Spinel.ctx_load_verify_locations(ctx, Spinel.string_value(ca)) unless ca.nil?

        cipher_filter = ssl_cipher_filter.nil? ? nil : Spinel.string_value(ssl_cipher_filter)
        ciphersuites = ssl_ciphersuites.nil? ? nil : Spinel.string_value(ssl_ciphersuites)
        reuse_cache_size = Spinel.num2int(reuse_cache_size) unless reuse.nil? || reuse_cache_size.nil?
        reuse_timeout = Spinel.num2int(reuse_timeout) unless reuse.nil? || reuse_timeout.nil?
        mode = verify_mode.nil? ? nil : Spinel.num2int(verify_mode)
        session_id_bytes = Random.bytes(32)
        ssl_raise Spinel.ctx_finish(ctx, no_tlsv1, no_tlsv1_1, reuse, reuse_cache_size,
          reuse_timeout, cipher_filter, ciphersuites, mode, session_id_bytes)
        freeze
        self
      end

      private def ssl_raise(message)
        raise SSLError, message unless message.nil?
      end
    end

    class Engine
      class << self
        # `rb_undef_alloc_func`: only `server` and `client` make one.
        def allocate
          raise TypeError, "allocator undefined for #{self}"
        end

        def new(*)
          raise TypeError, "allocator undefined for #{self}"
        end

        # `engine_init_server`: an engine accepting connections on `sslctx`,
        # a `Puma::MiniSSL::SSLContext`.
        def server(sslctx)
          unless SSLContext === sslctx
            raise TypeError, "wrong argument type #{Spinel.type_name(sslctx)} (expected MiniSSL/SSLContext)"
          end
          wrap(Spinel.engine_server(sslctx.__send__(:__ctx__)))
        end

        # `engine_init_client`.
        def client
          wrap(Spinel.engine_client)
        end

        private def wrap(handle)
          engine = Class.instance_method(:allocate).bind_call(self)
          engine.instance_variable_set(:@__engine__, handle)
          ObjectSpace.define_finalizer(engine, Spinel.finalizer(:engine_free, handle))
          engine
        end
      end

      # `engine_inject`: bytes from the peer, for OpenSSL to read.
      def inject(str)
        Spinel.engine_inject(@__engine__, Spinel.string_value(str))
      end

      # `engine_read`: decrypted bytes, nil when there are none yet.
      def read
        kind, fields = Spinel.records(Spinel.engine_read(@__engine__)).first
        case kind
        when 0x44 then fields[0] # D
        when 0x4e then nil # N
        when 0x4f then raise EOFError, "end of file reached" # O
        else raise SSLError, fields[0]
        end
      end

      # What one `SSL_write` can take of a String: Puma's context sets
      # `SSL_MODE_ENABLE_PARTIAL_WRITE`, so OpenSSL answers once it has
      # written one record (16 KiB), or a few at once where it pipelines
      # them, and no more is passed than that, where the C passes the
      # String's pointer: a copy of all of a 16 MiB body for each record
      # made writing it quadratic.
      WRITE_MAX = 128 * 1024

      # `engine_write`: bytes to encrypt; the count taken, nil when
      # OpenSSL wants to write first.
      def write(str)
        str = Spinel.string_value(str)
        str = str.byteslice(0, WRITE_MAX) if str.bytesize > WRITE_MAX
        answer = Spinel.engine_write(@__engine__, str)
        raise SSLError, answer if String === answer
        answer
      end

      # `engine_extract`: encrypted bytes for the peer, nil when there are
      # none.
      def extract
        kind, fields = Spinel.records(Spinel.engine_extract(@__engine__)).first
        case kind
        when 0x44 then fields[0] # D
        when 0x4e then nil # N
        else raise SSLError, fields[0]
        end
      end

      # `engine_shutdown`.
      def shutdown
        answer = Spinel.engine_shutdown(@__engine__)
        raise SSLError, answer if String === answer
        answer
      end

      # `engine_init`: whether the handshake is in progress.
      def init?
        Spinel.engine_init?(@__engine__)
      end

      # `engine_peercert`: the peer's certificate, DER encoded.
      def peercert
        Spinel.engine_peercert(@__engine__)&.force_encoding(Encoding::BINARY)
      end

      # `engine_ssl_vers_st`: the protocol and the handshake's state.
      def ssl_vers_st
        Spinel.records(Spinel.engine_ssl_vers_st(@__engine__)).first.last
      end
    end
  end
end
