# frozen_string_literal: true

# `ossl_ssl.c` and `ossl_ssl_session.c` on Spinel (#107): TLS over the
# system's OpenSSL 3, through `__openssl_tls__`. `lib/openssl/ssl.rb`, the
# Ruby half, then adds `set_params`, `SSLSocket`'s buffering and
# `post_connection_check`, and `SSLServer`, as upstream writes them.
#
# An SSLContext is an `SSL_CTX`, set up from its attributes the first time
# a socket uses it (`#setup`, which freezes it), as CRuby sets one up. An
# SSLSocket is an `SSL` on the IO's descriptor, which is made nonblocking:
# every call here is one nonblocking OpenSSL call, and a blocking `connect`,
# `accept`, `sysread` or `syswrite` waits on the IO between calls, so other
# threads run meanwhile, as they do around CRuby's waits.
#
# Where OpenSSL calls back into the program mid-handshake, the handshake is
# suspended and Ruby answers before it goes on:
#
# - certificate verification (`SSL_set_retry_verify`): the hostname check,
#   `OpenSSL::SSL.verify_certificate_identity`, and `verify_callback`, as
#   `ossl_ssl_verify_callback` runs them, on a client;
# - a server's ClientHello (`SSL_CTX_set_client_hello_cb`): `servername_cb`
#   and then `alpn_select_cb`;
# - a client's certificate request: `client_cert_cb`.
#
# What OpenSSL hands over as it goes, new sessions and key log lines, is
# given to `session_new_cb` and `keylog_cb` once the call returns.
#
# ponytail: the callbacks OpenSSL cannot suspend for are refused at setup
# with NotImplementedError rather than silently skipped, as skipping one can
# accept what the program meant to refuse: `session_get_cb`,
# `session_remove_cb`, `renegotiation_cb`, `tmp_dh_callback`, NPN, and a
# server's `verify_callback` (OpenSSL suspends verification only on a
# client). `tmp_dh=`, `enable_fallback_scsv`, `session_cache_stats`,
# `client_ca` and the sigalg and group readers are not here either.
# Upgrade: each one's OpenSSL call behind `__openssl_tls__`; the server's
# verification through the same replay, verifying the chain the client
# sent after the handshake's own check.

module OpenSSL
  module SSL
    class SSLError < OpenSSLError; end

    class SSLErrorWaitReadable < SSLError
      include IO::WaitReadable
    end

    class SSLErrorWaitWritable < SSLError
      include IO::WaitWritable
    end

    # `SSL_OP_*`, `SSL_VERIFY_*` and the protocol versions: OpenSSL 3's
    # `ssl.h`, `ssl3.h` and `tls1.h` numbers, the same in every OpenSSL 3
    # release. The options OpenSSL 3 no longer has are 0, as CRuby defines
    # them.
    {
      OP_ALL: 0x80000850, OP_LEGACY_SERVER_CONNECT: 0x4, OP_TLSEXT_PADDING: 0x10,
      OP_SAFARI_ECDHE_ECDSA_BUG: 0x40, OP_ALLOW_NO_DHE_KEX: 0x400,
      OP_DONT_INSERT_EMPTY_FRAGMENTS: 0x800, OP_NO_TICKET: 0x4000,
      OP_NO_SESSION_RESUMPTION_ON_RENEGOTIATION: 0x10000, OP_NO_COMPRESSION: 0x20000,
      OP_ALLOW_UNSAFE_LEGACY_RENEGOTIATION: 0x40000, OP_NO_ENCRYPT_THEN_MAC: 0x80000,
      OP_ENABLE_MIDDLEBOX_COMPAT: 0x100000, OP_PRIORITIZE_CHACHA: 0x200000,
      OP_CIPHER_SERVER_PREFERENCE: 0x400000, OP_TLS_ROLLBACK_BUG: 0x800000,
      OP_NO_ANTI_REPLAY: 0x1000000, OP_NO_SSLv3: 0x2000000, OP_NO_TLSv1: 0x4000000,
      OP_NO_TLSv1_2: 0x8000000, OP_NO_TLSv1_1: 0x10000000, OP_NO_TLSv1_3: 0x20000000,
      OP_NO_RENEGOTIATION: 0x40000000, OP_CRYPTOPRO_TLSEXT_BUG: 0x80000000,
      OP_CLEANSE_PLAINTEXT: 0x2, OP_ENABLE_KTLS: 0x8, OP_IGNORE_UNEXPECTED_EOF: 0x80,
      OP_ALLOW_CLIENT_RENEGOTIATION: 0x100, OP_DISABLE_TLSEXT_CA_NAMES: 0x200,
      OP_NO_SSLv2: 0, OP_MICROSOFT_SESS_ID_BUG: 0, OP_NETSCAPE_CHALLENGE_BUG: 0,
      OP_NETSCAPE_REUSE_CIPHER_CHANGE_BUG: 0, OP_SSLREF2_REUSE_CERT_TYPE_BUG: 0,
      OP_MICROSOFT_BIG_SSLV3_BUFFER: 0, OP_MSIE_SSLV2_RSA_PADDING: 0,
      OP_SSLEAY_080_CLIENT_DH_BUG: 0, OP_TLS_D5_BUG: 0, OP_TLS_BLOCK_PADDING_BUG: 0,
      OP_SINGLE_ECDH_USE: 0, OP_SINGLE_DH_USE: 0, OP_EPHEMERAL_RSA: 0, OP_PKCS1_CHECK_1: 0,
      OP_PKCS1_CHECK_2: 0, OP_NETSCAPE_CA_DN_BUG: 0, OP_NETSCAPE_DEMO_CIPHER_CHANGE_BUG: 0,
      VERIFY_NONE: 0, VERIFY_PEER: 1, VERIFY_FAIL_IF_NO_PEER_CERT: 2, VERIFY_CLIENT_ONCE: 4,
      SSL2_VERSION: 0x0002, SSL3_VERSION: 0x0300, TLS1_VERSION: 0x0301,
      TLS1_1_VERSION: 0x0302, TLS1_2_VERSION: 0x0303, TLS1_3_VERSION: 0x0304,
    }.each { |name, value| const_set(name, value) }

    # `parse_proto_version`'s names, in its order.
    PROTO_VERSIONS = [
      ["SSL2", SSL2_VERSION], ["SSL3", SSL3_VERSION], ["TLS1", TLS1_VERSION],
      ["TLS1_1", TLS1_1_VERSION], ["TLS1_2", TLS1_2_VERSION], ["TLS1_3", TLS1_3_VERSION],
    ].freeze
    private_constant :PROTO_VERSIONS

    def self.__tls__(*args) = OpenSSL.__send__(:__tls__, *args)
    private_class_method :__tls__

    class SSLContext
      SESSION_CACHE_OFF = 0x0
      SESSION_CACHE_CLIENT = 0x1
      SESSION_CACHE_SERVER = 0x2
      SESSION_CACHE_BOTH = 0x3
      SESSION_CACHE_NO_AUTO_CLEAR = 0x80
      SESSION_CACHE_NO_INTERNAL_LOOKUP = 0x100
      SESSION_CACHE_NO_INTERNAL_STORE = 0x200
      SESSION_CACHE_NO_INTERNAL = 0x300

      attr_accessor :cert, :key, :client_ca, :ca_file, :ca_path, :timeout, :verify_mode,
        :verify_depth, :verify_callback, :cert_store, :extra_chain_cert, :client_cert_cb,
        :session_id_context, :session_get_cb, :session_new_cb, :session_remove_cb,
        :renegotiation_cb, :alpn_protocols, :alpn_select_cb, :npn_protocols, :npn_select_cb,
        :keylog_cb, :verify_hostname, :tmp_dh_callback
      alias ssl_timeout timeout
      alias ssl_timeout= timeout=

      def options = __get__("options")

      # :call-seq:
      #    ctx.options = integer
      def options=(options)
        __check_frozen__
        __set__("options", options.nil? ? OP_ALL : Integer.__index__(options))
        self
      end

      # :call-seq:
      #    ctx.min_version = OpenSSL::SSL::TLS1_2_VERSION
      #    ctx.min_version = :TLS1_2
      #    ctx.min_version = nil
      def min_version=(version)
        __check_frozen__
        __set__("min_version", __proto__(version))
        version
      end

      def max_version=(version)
        __check_frozen__
        __set__("max_version", __proto__(version))
        version
      end

      # :call-seq:
      #    ctx.ciphers => [[name, version, bits, alg_bits], ...]
      def ciphers = __get__("ciphers")

      def ciphers=(ciphers)
        __check_frozen__
        return ciphers if ciphers.nil?
        __set__("ciphers", __cipher_string__(ciphers))
        ciphers
      end

      def ciphersuites=(ciphersuites)
        __check_frozen__
        return ciphersuites if ciphersuites.nil?
        __set__("ciphersuites", __cipher_string__(ciphersuites))
        ciphersuites
      end

      def sigalgs=(list)
        __check_frozen__
        __set__("sigalgs", OpenSSL.__send__(:__cstr__, list))
        list
      end

      def client_sigalgs=(list)
        __check_frozen__
        __set__("client_sigalgs", OpenSSL.__send__(:__cstr__, list))
        list
      end

      def ecdh_curves=(list)
        __check_frozen__
        __set__("ecdh_curves", OpenSSL.__send__(:__cstr__, list))
        list
      end

      def groups=(list)
        __check_frozen__
        __set__("groups", OpenSSL.__send__(:__cstr__, list))
        list
      end

      def security_level = __get__("security_level")

      def security_level=(level)
        __check_frozen__
        __set__("security_level", OpenSSL.__send__(:__int__, level))
        level
      end

      def session_cache_mode = __get__("session_cache_mode")

      def session_cache_mode=(mode)
        __set__("session_cache_mode", Integer.__index__(mode))
        mode
      end

      def session_cache_size = __get__("session_cache_size")

      def session_cache_size=(size)
        __set__("session_cache_size", Integer.__index__(size))
        size
      end

      # :call-seq:
      #    ctx.session_add(session) -> true | false
      def session_add(session)
        SSL.__send__(:__tls__, "ctx_session", __ctx_id__, "add", Session.__send__(:__id_of__, session))
      end

      def session_remove(session)
        SSL.__send__(:__tls__, "ctx_session", __ctx_id__, "remove", Session.__send__(:__id_of__, session))
      end

      def flush_sessions(time = nil)
        time = time.nil? ? Time.now.to_i : Integer(time)
        SSL.__send__(:__tls__, "ctx_session", __ctx_id__, "flush", time)
        self
      end

      # :call-seq:
      #    ctx.add_certificate(certificate, pkey [, extra_certs]) -> self
      def add_certificate(*args)
        unless args.size == 2 || args.size == 3
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 2..3)"
        end
        cert, key, extra = args
        __check_frozen__
        cert_id = X509.__send__(:__cert_id__, cert)
        key_id = OpenSSL.__send__(:__private_key__, key)
        extra_ids = args.size == 3 ? Array(extra).map { |c| X509.__send__(:__cert_id__, c) } : nil
        SSL.__send__(:__tls__, "ctx_add_certificate", __ctx_id__, cert_id, key_id, extra_ids)
        self
      end

      def tmp_dh=(dh)
        raise NotImplementedError, "OpenSSL::SSL::SSLContext#tmp_dh= is not supported on Spinel yet"
      end

      # :call-seq:
      #    ctx.setup => Qtrue # first time
      #    ctx.setup => nil # thereafter
      #
      # This method is called automatically when a new SSLSocket is created.
      # It freezes the context once it is set up, so nothing set after it can
      # be lost; a setup that raises leaves it unfrozen, to be tried again.
      def setup
        return nil if frozen?
        ctx_id = __ctx_id__
        {
          session_get_cb: "session_get_cb", session_remove_cb: "session_remove_cb",
          renegotiation_cb: "renegotiation_cb", tmp_dh_callback: "tmp_dh_callback",
          npn_protocols: "npn_protocols", npn_select_cb: "npn_select_cb",
        }.each do |ivar, name|
          next if instance_variable_get(:"@#{ivar}").nil?
          raise NotImplementedError, "OpenSSL::SSL::SSLContext##{name} is not supported on Spinel yet"
        end
        store = @cert_store.nil? ? nil : __store_id__(@cert_store)
        extra = []
        @extra_chain_cert&.each { |c| extra << X509.__send__(:__cert_id__, c) }
        cert = @cert.nil? ? nil : X509.__send__(:__cert_id__, @cert)
        key = @key.nil? ? nil : OpenSSL.__send__(:__private_key__, @key)
        cas = []
        unless @client_ca.nil?
          (Array === @client_ca ? @client_ca : [@client_ca]).each { |c| cas << X509.__send__(:__cert_id__, c) }
        end
        ca_file = @ca_file.nil? ? nil : OpenSSL.__send__(:__cstr__, @ca_file)
        ca_path = @ca_path.nil? ? nil : OpenSSL.__send__(:__cstr__, @ca_path)
        verify_mode = @verify_mode.nil? ? nil : OpenSSL.__send__(:__int__, @verify_mode)
        timeout = @timeout.nil? ? nil : Integer.__index__(@timeout)
        depth = @verify_depth.nil? ? nil : OpenSSL.__send__(:__int__, @verify_depth)
        alpn = @alpn_protocols.nil? ? nil : __alpn_wire__(@alpn_protocols)
        sid = @session_id_context.nil? ? nil : IO.__str__(@session_id_context)
        SSL.__send__(:__tls__, "ctx_setup", ctx_id, store, extra, cert, key, cas, ca_file, ca_path,
          verify_mode, timeout, depth, alpn, sid, !@session_new_cb.nil?, !@keylog_cb.nil?,
          !(@servername_cb.nil? && @alpn_select_cb.nil?), !@alpn_select_cb.nil?, !@client_cert_cb.nil?)
        Object.instance_method(:freeze).bind_call(self)
        true
      end
      alias freeze setup

      private

      # The `SSL_CTX`, made the first time it is needed, which is before
      # `setup` freezes the context; CRuby's allocator makes it.
      def __ctx_id__
        holder = @__ctx__
        return holder[0] unless holder.nil?
        @__ctx__ = OpenSSL.__send__(:__holder__, SSL.__send__(:__tls__, "ctx_new"))
        @__ctx__[0]
      end

      # A copy is a new `SSL_CTX` with the original's attributes, as CRuby's
      # `dup` allocates one.
      def initialize_copy(other)
        super
        @__ctx__ = nil
        self
      end

      def __get__(field) = SSL.__send__(:__tls__, "ctx_get", __ctx_id__, field)
      def __set__(field, value) = SSL.__send__(:__tls__, "ctx_set", __ctx_id__, field, value)

      def __check_frozen__
        return unless frozen?
        raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self)
      end

      def __store_id__(store)
        unless OpenSSL::X509::Store === store
          raise TypeError, "wrong argument type #{store.class} (expected OpenSSL/X509/STORE)"
        end
        store.__send__(:__store_id__)
      end

      # `parse_proto_version`: nil, an Integer, or a name (`:TLS1_2`), which
      # is the first of its table to start with the String given.
      def __proto__(version)
        return 0 if version.nil?
        return OpenSSL.__send__(:__int__, version) if Integer === version
        version = version.to_s if Symbol === version
        version = IO.__str__(version)
        found = PROTO_VERSIONS.find { |name, _| name.start_with?(version) }
        raise ArgumentError, "unrecognized version #{version.inspect}" if found.nil?
        found[1]
      end

      # `build_cipher_string`: an Array's names joined with `:`.
      def __cipher_string__(value)
        return value.map { |c| Array === c ? c[0] : c }.join(":") if Array === value
        OpenSSL.__send__(:__cstr__, value)
      end

      # `ssl_encode_npn_protocols`: each protocol's length and name.
      def __alpn_wire__(protocols)
        out = +"".b
        protocols.each do |protocol|
          protocol = IO.__str__(protocol)
          unless (1..255).cover?(protocol.bytesize)
            raise SSLError, "Advertised protocol must have length 1..255"
          end
          out << protocol.bytesize.chr << protocol.b
        end
        out
      end
    end

    class SSLSocket
      # :call-seq:
      #    SSLSocket.new(io) => aSSLSocket
      #    SSLSocket.new(io, ctx) => aSSLSocket
      def initialize(*args)
        unless args.size == 1 || args.size == 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
        end
        io, ctx = args
        ctx = SSLContext.new if ctx.nil?
        unless SSLContext === ctx
          raise TypeError, "wrong argument type #{ctx.class} (expected OpenSSL/SSL/CTX)"
        end
        @context = ctx
        ctx.setup
        io.nonblock = true if io.respond_to?(:nonblock=)
        raise TypeError, "wrong argument type #{io.class} (expected File)" unless IO === io
        @io = io
        @__verify_callback__ = ctx.verify_callback
        id = SSL.__send__(:__tls__, "ssl_new", ctx.__send__(:__ctx_id__), !ctx.verify_callback.nil?,
          ctx.verify_hostname ? true : false)
        @__ssl__ = OpenSSL.__send__(:__holder__, id)
        super()
      end

      # :call-seq:
      #    ssl.connect => self
      #
      # Initiates an SSL/TLS handshake with a server.
      def connect = __start__(false, false, true)

      # :call-seq:
      #    ssl.connect_nonblock([options]) => self
      def connect_nonblock(exception: true) = __start__(false, true, exception)

      # :call-seq:
      #    ssl.accept => self
      #
      # Waits for a SSL/TLS client to initiate a handshake.
      def accept = __start__(true, false, true)

      def accept_nonblock(exception: true) = __start__(true, true, exception)

      # :call-seq:
      #    ssl.sysread(length) => string
      #    ssl.sysread(length, buffer) => buffer
      def sysread(*args)
        unless args.size == 1 || args.size == 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
        end
        __read__(args[0], args[1], false, true)
      end

      # :call-seq:
      #    ssl.syswrite(string) => Integer
      def syswrite(str) = __write__(str, false, true)

      def hostname=(hostname)
        name = hostname.nil? ? nil : OpenSSL.__send__(:__cstr__, hostname)
        SSL.__send__(:__tls__, "ssl_hostname", __ssl_id__, name)
        @hostname = hostname
      end

      def cert
        id = __get__("cert")
        id && X509.__send__(:__cert__, id)
      end

      def peer_cert
        id = __get__("peer_cert")
        id && X509.__send__(:__cert__, id)
      end

      def peer_cert_chain
        ids = __get__("peer_cert_chain")
        ids&.map { |i| X509.__send__(:__cert__, i) }
      end

      def ssl_version = __get__("version")

      def cipher = __get__("cipher")

      def state = __get__("state")

      def pending = __get__("pending")

      def session_reused? = __get__("session_reused")

      def verify_result = __get__("verify_result")

      def alpn_protocol = __get__("alpn_protocol")

      def npn_protocol = nil

      def finished_message = __get__("finished_message")

      def peer_finished_message = __get__("peer_finished_message")

      def tmp_key
        pair = __get__("tmp_key")
        pair && OpenSSL::PKey.__send__(:__wrap__, pair)
      end

      # :call-seq:
      #    ssl.session = session -> session
      def session=(session)
        SSL.__send__(:__tls__, "ssl_set_session", __ssl_id__, Session.__send__(:__id_of__, session))
        session
      end

      # :call-seq:
      #    ssl.export_keying_material(label, length) -> String
      #    ssl.export_keying_material(label, length, context) -> String
      def export_keying_material(label, length, context = nil)
        SSL.__send__(:__tls__, "ssl_export_keying_material", __ssl_id__, IO.__str__(label),
          Integer.__index__(length), context.nil? ? nil : IO.__str__(context))
      end

      private

      def __ssl_id__ = @__ssl__[0]

      def __get__(field) = SSL.__send__(:__tls__, "ssl_get", __ssl_id__, field)

      def stop
        SSL.__send__(:__tls__, "ssl_shutdown", __ssl_id__)
        nil
      end

      def sysread_nonblock(*args, exception: true)
        unless args.size == 1 || args.size == 2
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)"
        end
        __read__(args[0], args[1], true, exception)
      end

      def syswrite_nonblock(str, exception: true) = __write__(str, true, exception)

      # `ossl_ssl_setup`: the descriptor, once, from an IO open both ways.
      def __setup__
        io = @io
        raise IOError, "closed stream" if io.closed?
        io.__send__(:__readable__)
        io.__send__(:__writable__)
        SSL.__send__(:__tls__, "ssl_setup", __ssl_id__, io.fileno)
      end

      # `io_wait_readable`, `io_wait_writable`: the IO's own timeout.
      def __wait__(readable)
        events = readable ? IO::READABLE : IO::WRITABLE
        return unless IO.__wait__([[@io, events]], @io.timeout, true).nil?
        raise IO::TimeoutError, "Timed out while waiting to become #{readable ? "readable" : "writable"}!"
      end

      # `ossl_start_ssl`: `SSL_connect` or `SSL_accept` until done, Ruby
      # answering what the handshake asks on the way.
      def __start__(server, nonblock, exception)
        __setup__
        if server && !@__verify_callback__.nil? && (@context.verify_mode.to_i & VERIFY_PEER) != 0
          raise NotImplementedError, "a server's verify_callback is not supported on Spinel yet"
        end
        func = server ? "SSL_accept" : "SSL_connect"
        loop do
          r = SSL.__send__(:__tls__, "ssl_handshake", __ssl_id__, server)
          __callbacks__
          __raise_callback_error__
          case r
          when true then return self
          when :wait_readable
            return :wait_readable if nonblock && !exception
            raise SSLErrorWaitReadable, "read would block" if nonblock
            __wait__(true)
          when :wait_writable
            return :wait_writable if nonblock && !exception
            raise SSLErrorWaitWritable, "write would block" if nonblock
            __wait__(false)
          else
            case r[0]
            when :verify then __verify__(r[1], server)
            when :hello then __hello__(r[1], r[2])
            when :client_cert then __client_cert__
            when :syserr then raise SystemCallError.new(r[2], r[1])
            else __handshake_error__(func, r)
            end
          end
        end
      end

      def __handshake_error__(func, failure)
        _, code, errno, state, tail, errors = failure
        peer =
          begin
            @io.remote_address.inspect_sockaddr
          rescue SystemCallError
            "(null)"
          end
        message = "#{func}#{code == 5 ? " SYSCALL" : ""} returned=#{code} errno=#{errno} " \
          "peeraddr=#{peer} state=#{state}#{tail}"
        error = SSLError.new(message)
        error.instance_variable_set(:@errors, errors)
        raise error
      end

      # What a Ruby callback raised, raised once the OpenSSL call it ran
      # under has returned, as CRuby re-raises its callback state.
      def __raise_callback_error__
        error = @__callback_error__
        return if error.nil?
        @__callback_error__ = nil
        raise error
      end

      # `session_new_cb` and `keylog_cb`, for what OpenSSL handed over.
      def __callbacks__
        ctx = @context
        return if ctx.session_new_cb.nil? && ctx.keylog_cb.nil?
        _, sessions, lines = SSL.__send__(:__tls__, "ssl_take", __ssl_id__)
        sessions.each do |id|
          session = Session.__send__(:__wrap__, id)
          ctx.session_new_cb&.call([self, session])
        end
        lines.each { |line| ctx.keylog_cb&.call(self, line) }
      rescue Exception => e
        @__callback_error__ ||= e
      end

      # `ossl_ssl_verify_callback`, for the question a suspended
      # verification asks: the hostname check on a client's leaf, then the
      # context's `verify_callback`.
      def __verify__(question, server)
        ok, error, depth, der = question
        if ok && @context.verify_hostname && !server && depth == 0
          begin
            hostname = @hostname
            if hostname
              identity = OpenSSL::SSL.verify_certificate_identity(X509::Certificate.new(der), hostname)
            else
              warn "verify_hostname requires hostname to be set" if $VERBOSE
              identity = true
            end
          rescue Exception => e
            @__callback_error__ = e
            SSL.__send__(:__tls__, "ssl_answer", __ssl_id__, false, error)
            return
          end
          unless identity == true
            ok = false
            error = X509::V_ERR_HOSTNAME_MISMATCH
          end
        end
        ok, error = X509::StoreContext.__answer__(@__verify_callback__, question, ok, error)
        SSL.__send__(:__tls__, "ssl_answer", __ssl_id__, ok, error)
      end

      # A server's ClientHello: `servername_cb` for the host name a client
      # gave, then `alpn_select_cb` for the protocols it offers.
      def __hello__(servername, protocols)
        ctx = @context
        chosen = nil
        alpn = nil
        # The alert a failure sends, OpenSSL's for each callback under CRuby:
        # unrecognized_name for servername_cb, no_application_protocol for
        # alpn_select_cb.
        alert = 112
        begin
          if servername && ctx.servername_cb
            result = ctx.servername_cb.call([self, servername])
            if SSLContext === result
              result.setup
              @context = result
              chosen = result.__send__(:__ctx_id__)
              ctx = result
            elsif !result.nil?
              raise ArgumentError, "servername_cb must return an OpenSSL::SSL::SSLContext object or nil"
            end
          end
          if protocols && ctx.alpn_select_cb
            alert = 120
            selected = IO.__str__(ctx.alpn_select_cb.call(protocols))
            unless (1..255).cover?(selected.bytesize)
              raise SSLError, "Selected protocol name must have length 1..255"
            end
            alpn = selected
          end
        rescue Exception => e
          @__callback_error__ = e
          SSL.__send__(:__tls__, "ssl_hello_answer", __ssl_id__, alert, nil, nil)
          return
        end
        SSL.__send__(:__tls__, "ssl_hello_answer", __ssl_id__, nil, chosen, alpn)
      end

      # `ossl_client_cert_cb`: `[cert, key]` from `client_cert_cb`, or none.
      def __client_cert__
        cert = key = nil
        begin
          result = @context.client_cert_cb.call(self)
          if Array === result && result.size == 2
            cert = X509.__send__(:__cert_id__, result[0])
            key = OpenSSL.__send__(:__private_key__, result[1])
          end
        rescue Exception => e
          @__callback_error__ = e
        end
        SSL.__send__(:__tls__, "ssl_client_cert", __ssl_id__, cert, key)
      end

      # `ossl_ssl_read_internal`.
      def __read__(len, buf, nonblock, exception)
        len = OpenSSL.__send__(:__int__, len)
        raise ArgumentError, "negative length #{len} given" if len.negative?
        buf = IO.__str__(buf) unless buf.nil?
        if len.zero?
          return buf.nil? ? +"".b : buf.replace("")
        end
        raise SSLError, "SSL session is not started yet" unless __get__("started")
        loop do
          r = SSL.__send__(:__tls__, "ssl_read", __ssl_id__, len)
          __callbacks__
          __raise_callback_error__
          case r
          when String
            return r if buf.nil?
            return buf.replace(r.force_encoding(buf.encoding))
          when :eof
            return nil if nonblock && !exception
            raise EOFError, "end of file reached"
          when :wait_readable
            if nonblock
              return :wait_readable unless exception
              raise SSLErrorWaitReadable, "read would block"
            end
            __wait__(true)
          when :wait_writable
            if nonblock
              return :wait_writable unless exception
              raise SSLErrorWaitWritable, "write would block"
            end
            __wait__(false)
          else
            raise SystemCallError.new(nil, r[1]) if r[0] == :syserr
            __verify__(r[1], false) if r[0] == :verify
          end
        end
      end

      # `ossl_ssl_write_internal`.
      def __write__(str, nonblock, exception)
        str = IO.__str__(str)
        raise SSLError, "SSL session is not started yet" unless __get__("started")
        loop do
          r = SSL.__send__(:__tls__, "ssl_write", __ssl_id__, str)
          case r
          when Integer then return r
          when :wait_writable
            return :wait_writable if nonblock && !exception
            raise SSLErrorWaitWritable, "write would block" if nonblock
            __wait__(false)
          when :wait_readable
            return :wait_readable if nonblock && !exception
            raise SSLErrorWaitReadable, "read would block" if nonblock
            __wait__(true)
          else
            raise SystemCallError.new(nil, r[1]) if r[0] == :syserr
          end
        end
      end
    end

    class Session
      class SessionError < OpenSSLError; end

      def self.__wrap__(id)
        session = allocate
        session.instance_variable_set(:@__session__, OpenSSL.__send__(:__holder__, id))
        session
      end

      def self.__id_of__(session)
        raise TypeError, "wrong argument type #{session.class} (expected OpenSSL/SSL/Session)" unless Session === session
        session.__send__(:__session_id__)
      end
      private_class_method :__wrap__, :__id_of__

      # :call-seq:
      #    Session.new(ssl_socket) -> Session
      #    Session.new(string) -> Session
      def initialize(arg)
        id =
          if SSLSocket === arg
            arg.__send__(:__get__, "session") or raise SessionError, "no session available"
          else
            arg = arg.to_der if arg.respond_to?(:to_der)
            SSL.__send__(:__tls__, "session_read", IO.__str__(arg))
          end
        @__session__ = OpenSSL.__send__(:__holder__, id)
      end

      def time
        t = __get__("time")
        t.zero? ? nil : Time.at(t)
      end

      def time=(time)
        __set__("time", Integer(time))
        time
      end

      def timeout = __get__("timeout")

      def timeout=(seconds)
        __set__("timeout", Integer.__index__(seconds))
        seconds
      end

      def id = __get__("id")

      def to_der = __get__("der")
      def to_pem = __get__("pem")
      def to_text = __get__("text")

      def ==(other)
        return false unless Session === other
        __get__("protocol") == other.__send__(:__get__, "protocol") && id == other.id
      end

      private

      def __session_id__ = @__session__[0]
      def __get__(field) = SSL.__send__(:__tls__, "session_get", __session_id__, field)
      def __set__(field, value) = SSL.__send__(:__tls__, "session_set", __session_id__, field, value)
    end
  end
end
