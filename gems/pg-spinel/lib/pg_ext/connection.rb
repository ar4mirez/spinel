# frozen_string_literal: true

# pg_connection.c: PG::Connection's C methods, which upstream's
# `lib/pg/connection.rb` builds `new`, the `async_*` aliases, `transaction`
# and the rest on. Its libpq connection is a box held by handle in
# `@__pg_conn__`; the IO over its socket, the encoding its strings are in
# and its type maps are the struct's other fields.

require "socket"

module PG
  class Connection
    include Constants

    # pgconn_s_allocate.
    def self.allocate
      conn = super
      handle = Spinel.conn_new
      conn.instance_variable_set(:@__pg_conn__, handle)
      ObjectSpace.define_finalizer(conn, Spinel.conn_finalizer(handle))
      conn.instance_variable_set(:@calls_to_put_copy_data, 0)
      conn.instance_variable_set(:@iopts_for_reset, nil)
      conn
    end

    class << self
      # The connection string `parse_connect_args` makes of `args`.
      private def __pg_conninfo__(args)
        Spinel.c_string(parse_connect_args(*args))
      end

      def sync_connect(*args)
        conn = allocate
        h = conn.__send__(:__pg_handle__)
        conninfo = __pg_conninfo__(args)
        raise PG::Error, "PQconnectdb() unable to allocate PGconn structure" unless Spinel.conn_connectdb(h, conninfo)
        if Spinel.conn_status(h) == Constants::CONNECTION_BAD
          Spinel.raise_conn_error(ConnectionBad, conn, Spinel.conn_error_message(h))
        end
        conn.set_default_encoding
        return conn unless block_given?
        begin
          yield conn
        ensure
          conn.finish
        end
      end

      def connect_start(*args)
        conn = allocate
        h = conn.__send__(:__pg_handle__)
        conninfo = __pg_conninfo__(args)
        raise PG::Error, "PQconnectStart() unable to allocate PGconn structure" unless Spinel.conn_connect_start(h, conninfo)
        if Spinel.conn_status(h) == Constants::CONNECTION_BAD
          Spinel.raise_conn_error(ConnectionBad, conn, Spinel.conn_error_message(h))
        end
        return conn unless block_given?
        begin
          yield conn
        ensure
          conn.finish
        end
      end

      def sync_ping(*args)
        Spinel.ping(__pg_conninfo__(args))
      end

      def conndefaults
        packed = Spinel.conndefaults
        packed && Spinel.conninfo_array(packed)
      end

      def conninfo_parse(conninfo)
        errmsg, *rest = Spinel.unpack(Spinel.conninfo_parse(Spinel.c_string(conninfo)))
        raise PG::Error, errmsg.to_s if errmsg
        Spinel.conninfo_array(Spinel.pack(rest))
      end

      def escape_string(string)
        string = Spinel.c_string(string)
        Spinel.escape_string(string).force_encoding(string.encoding)
      end
      alias_method :escape, :escape_string

      def escape_bytea(str)
        Spinel.escape_bytea(Spinel.check_string(str)).b
      end

      def unescape_bytea(str)
        Spinel.unescape_bytea(Spinel.c_string(Spinel.check_string(str))).b
      end

      def encrypt_password(password, username)
        Spinel.check_string(password)
        Spinel.check_string(username)
        Spinel.encrypt_password(Spinel.c_string(password), Spinel.c_string(username)).b
      end

      def quote_ident(str_or_array)
        enc = String === str_or_array ? str_or_array.encoding : Encoding::BINARY
        Spinel.quote_identifier(str_or_array, enc)
      end
    end

    # pg_get_connection: the box's handle.
    private def __pg_handle__
      @__pg_conn__
    end

    # pg_get_connection_safe: the handle of an open, unfrozen connection.
    private def __pg_safe__
      Spinel.check_frozen(self)
      h = @__pg_conn__
      Spinel.raise_conn_error(ConnectionBad, self, "connection is closed") unless Spinel.conn_open?(h)
      h
    end

    # pg_get_pgconn: the handle of an open connection.
    private def __pg_conn__
      h = @__pg_conn__
      Spinel.raise_conn_error(ConnectionBad, self, "connection is closed") unless Spinel.conn_open?(h)
      h
    end

    # The encoding the connection's strings are in (`enc_idx`).
    private def __pg_enc__
      @__pg_enc__ || Encoding::BINARY
    end

    # pgconn_close_socket_io.
    private def __pg_close_socket_io__
      if (io = @__pg_socket_io__)
        io.close
      end
      @__pg_socket_io__ = nil
    end

    # pg_cstr_enc: `str` as a C string in the connection's encoding.
    private def __pg_cstr__(str)
      Spinel.export(Spinel.c_string(str), __pg_enc__)
    end

    # pg_new_result.
    private def __pg_result__(handle)
      Result.__send__(:__pg_new__, handle, self)
    end

    # pg_new_result, then pg_result_check.
    private def __pg_checked_result__(handle)
      result = __pg_result__(handle)
      result.check
      result
    end

    # `rb_ensure(rb_yield, result, pg_result_clear, result)` with a block.
    private def __pg_yield_result__(result)
      return result unless block_given?
      begin
        yield result
      ensure
        result.clear
      end
    end

    # alloc_query_params: values, types and formats packed for the
    # extension.
    private def __pg_params__(params, with_types, typemap)
      Spinel.check_array(params)
      typemap.__pg_fit_to_query__(params)
      enc = __pg_enc__
      values = []
      types = []
      formats = []
      params.each do |value|
        format = 0
        type = 0
        if Hash === value
          format_value = value[:format]
          format = Spinel.num2int(format_value) unless format_value.nil?
          if with_types
            type_value = value[:type]
            type = Spinel.num2uint(type_value) unless type_value.nil?
          end
          value = value[:value]
        end
        formats << format
        types << type
        if value.nil?
          values << nil
        else
          str = Spinel.export(Spinel.as_string(value), enc)
          Spinel.c_string(str) if format == 0
          values << str
        end
      end
      [Spinel.pack(values), with_types ? types.pack("L<*") : nil, formats.pack("l<*")]
    end

    # pgconn_query_assign_typemap.
    private def __pg_query_typemap__(typemap)
      typemap.nil? ? type_map_for_queries : Spinel.check_type_map(typemap)
    end

    # The parameter types `prepare` takes: nil, or an Array of Oids and nils.
    private def __pg_param_types__(param_types)
      return nil if param_types.nil?
      Spinel.check_array(param_types).map { |t| t.nil? ? 0 : Spinel.num2uint(t) }.pack("L<*")
    end

    def connect_poll
      __pg_close_socket_io__
      Spinel.conn_connect_poll(__pg_conn__)
    end

    def finish
      h = __pg_safe__
      __pg_close_socket_io__
      Spinel.conn_finish(h)
      nil
    end
    alias_method :close, :finish

    def finished?
      !Spinel.conn_open?(@__pg_conn__)
    end

    def sync_reset
      __pg_close_socket_io__
      Spinel.conn_reset(__pg_conn__)
      self
    end

    private def reset_start2(conninfo)
      conninfo = Spinel.c_string(conninfo)
      __pg_close_socket_io__
      h = @__pg_conn__
      unless Spinel.conn_connect_start(h, conninfo)
        raise PG::Error, "PQconnectStart() unable to allocate PGconn structure"
      end
      if Spinel.conn_status(h) == CONNECTION_BAD
        Spinel.raise_conn_error(ConnectionBad, self, Spinel.conn_error_message(h))
      end
      nil
    end

    def reset_start
      __pg_close_socket_io__
      Spinel.raise_conn_error(UnableToSend, self, "reset has failed") if Spinel.conn_reset_start(__pg_conn__) == 0
      nil
    end

    def reset_poll
      __pg_close_socket_io__
      Spinel.conn_reset_poll(__pg_conn__)
    end

    def db
      Spinel.conn_db(__pg_conn__)&.b
    end

    def user
      Spinel.conn_user(__pg_conn__)&.b
    end

    def pass
      Spinel.conn_pass(__pg_conn__)&.b
    end

    def host
      Spinel.conn_host(__pg_conn__)&.b
    end

    if Spinel::RESULT_MEMORY_SIZE
      def hostaddr
        Spinel.conn_hostaddr(__pg_conn__)&.b
      end
    end

    def port
      port = Spinel.conn_port(__pg_conn__)
      port.nil? || port.empty? ? DEF_PGPORT : port.to_i
    end

    def tty
      "".b
    end

    def conninfo
      packed = Spinel.conn_conninfo(__pg_conn__)
      packed && Spinel.conninfo_array(packed)
    end

    def options
      Spinel.conn_options(__pg_conn__)&.b
    end

    def status
      Spinel.conn_status(__pg_conn__)
    end

    def transaction_status
      Spinel.conn_transaction_status(__pg_conn__)
    end

    def parameter_status(param_name)
      h = __pg_conn__
      Spinel.conn_parameter_status(h, Spinel.c_string(param_name))&.b
    end

    def protocol_version
      version = Spinel.conn_protocol_version(__pg_conn__)
      Spinel.raise_conn_error(ConnectionBad, self, "PQprotocolVersion() can't get protocol version") if version == 0
      version
    end

    if Spinel::FULL_PROTOCOL_VERSION
      def full_protocol_version
        version = Spinel.conn_full_protocol_version(__pg_conn__)
        Spinel.raise_conn_error(ConnectionBad, self, "PQfullProtocolVersion() can't get protocol version") if version == 0
        version
      end
    end

    def server_version
      version = Spinel.conn_server_version(__pg_conn__)
      Spinel.raise_conn_error(ConnectionBad, self, "PQserverVersion() can't get server version") if version == 0
      version
    end

    def error_message
      Spinel.conn_error_message(__pg_conn__)&.b
    end

    def socket
      Spinel.deprecated(4, "conn.socket is deprecated and should be replaced by conn.socket_io")
      sd = Spinel.conn_socket(__pg_conn__)
      Spinel.raise_conn_error(ConnectionBad, self, "PQsocket() can't get socket descriptor") if sd < 0
      sd
    end

    def socket_io
      h = __pg_safe__
      return @__pg_socket_io__ if @__pg_socket_io__
      sd = Spinel.conn_socket(h)
      Spinel.raise_conn_error(ConnectionBad, self, "PQsocket() can't get socket descriptor") if sd < 0
      io = BasicSocket.for_fd(sd)
      io.autoclose = false
      @__pg_socket_io__ = io
    end

    def backend_pid
      Spinel.conn_backend_pid(__pg_conn__)
    end

    unless Spinel::CHUNK_MODE
      def backend_key
        key = Spinel.conn_backend_key(__pg_conn__)
        Spinel.raise_conn_error(PG::Error, self, "Invalid connection!") if key.nil?
        raise PG::Error, "Unexpected binary struct layout - please file a bug report at ruby-pg!" if key == false
        key
      end
    end

    def connection_needs_password
      Spinel.conn_needs_password(__pg_conn__)
    end

    def connection_used_password
      Spinel.conn_used_password(__pg_conn__)
    end

    def sync_exec(*args, &block)
      h = __pg_safe__
      if args.length == 1 || (args.length >= 2 && args.length <= 4 && args[1].nil?)
        sql = __pg_cstr__(args[0])
        result = __pg_checked_result__(Spinel.conn_exec(h, sql))
        return __pg_yield_result__(result, &block)
      end
      Spinel.deprecated(0, "forwarding exec to exec_params is deprecated")
      sync_exec_params(*args, &block)
    end

    def sync_exec_params(command, params = nil, result_format = nil, typemap = nil, &block)
      h = __pg_safe__
      if params.nil?
        Spinel.deprecated(1, "forwarding exec_params to exec is deprecated")
        return sync_exec(command, &block)
      end
      typemap = __pg_query_typemap__(typemap)
      format = result_format.nil? ? 0 : Spinel.num2int(result_format)
      values, types, formats = __pg_params__(params, true, typemap)
      sql = __pg_cstr__(command)
      result = __pg_checked_result__(Spinel.conn_exec_params(h, sql, values, types, formats, format))
      __pg_yield_result__(result, &block)
    end

    def sync_prepare(name, command, param_types = nil)
      h = __pg_safe__
      name = __pg_cstr__(name)
      command = __pg_cstr__(command)
      __pg_checked_result__(Spinel.conn_prepare(h, name, command, __pg_param_types__(param_types)))
    end

    def sync_exec_prepared(name, params = nil, result_format = nil, typemap = nil, &block)
      h = __pg_safe__
      params = [] if params.nil?
      typemap = __pg_query_typemap__(typemap)
      format = result_format.nil? ? 0 : Spinel.num2int(result_format)
      values, _, formats = __pg_params__(params, false, typemap)
      name = __pg_cstr__(name)
      result = __pg_checked_result__(Spinel.conn_exec_prepared(h, name, values, formats, format))
      __pg_yield_result__(result, &block)
    end

    def sync_describe_prepared(stmt_name)
      h = __pg_safe__
      __pg_checked_result__(Spinel.conn_describe_prepared(h, stmt_name.nil? ? nil : __pg_cstr__(stmt_name)))
    end

    def sync_describe_portal(stmt_name)
      h = __pg_safe__
      __pg_checked_result__(Spinel.conn_describe_portal(h, stmt_name.nil? ? nil : __pg_cstr__(stmt_name)))
    end

    if Spinel::CHUNK_MODE
      def sync_close_prepared(stmt_name)
        h = __pg_safe__
        __pg_checked_result__(Spinel.conn_close_prepared(h, stmt_name.nil? ? nil : __pg_cstr__(stmt_name)))
      end

      def sync_close_portal(stmt_name)
        h = __pg_safe__
        __pg_checked_result__(Spinel.conn_close_portal(h, stmt_name.nil? ? nil : __pg_cstr__(stmt_name)))
      end
    end

    # pgconn_async_exec and the others: discard what is pending, send, and
    # wait for the last result.
    def exec(*args, &block)
      discard_results
      send_query(*args)
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    def exec_params(*args, &block)
      discard_results
      if args.length == 1 || (args.length >= 2 && args.length <= 4 && args[1].nil?)
        Spinel.deprecated(3, "forwarding async_exec_params to async_exec is deprecated")
        send_query(*args)
      else
        send_query_params(*args)
      end
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    def prepare(*args, &block)
      discard_results
      send_prepare(*args)
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    def exec_prepared(*args, &block)
      discard_results
      send_query_prepared(*args)
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    def describe_prepared(stmt_name, &block)
      discard_results
      send_describe_prepared(stmt_name)
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    def describe_portal(portal, &block)
      discard_results
      send_describe_portal(portal)
      __pg_yield_result__(__pg_async_get_last_result__, &block)
    end

    if Spinel::CHUNK_MODE
      def close_prepared(stmt_name, &block)
        discard_results
        send_close_prepared(stmt_name)
        __pg_yield_result__(__pg_async_get_last_result__, &block)
      end

      def close_portal(portal, &block)
        discard_results
        send_close_portal(portal)
        __pg_yield_result__(__pg_async_get_last_result__, &block)
      end
    end

    alias_method :async_exec, :exec
    alias_method :async_query, :async_exec
    alias_method :async_exec_params, :exec_params
    alias_method :async_prepare, :prepare
    alias_method :async_exec_prepared, :exec_prepared
    alias_method :async_describe_prepared, :describe_prepared
    alias_method :async_describe_portal, :describe_portal
    if Spinel::CHUNK_MODE
      alias_method :async_close_prepared, :close_prepared
      alias_method :async_close_portal, :close_portal
    end

    def make_empty_pgresult(status)
      h = __pg_conn__
      __pg_checked_result__(Spinel.conn_make_empty_result(h, Spinel.num2int(status)))
    end

    def escape_string(string)
      string = Spinel.c_string(string)
      enc = __pg_enc__
      string = Spinel.export(string, enc)
      h = __pg_conn__
      escaped = Spinel.conn_escape_string(h, string)
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if escaped.nil?
      escaped.force_encoding(enc)
    end
    alias_method :escape, :escape_string

    def escape_literal(string)
      h = __pg_safe__
      string = Spinel.c_string(string)
      enc = __pg_enc__
      escaped = Spinel.conn_escape_literal(h, Spinel.export(string, enc))
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if escaped.nil?
      escaped.force_encoding(enc)
    end

    def escape_identifier(string)
      h = __pg_safe__
      string = Spinel.c_string(string)
      enc = __pg_enc__
      escaped = Spinel.conn_escape_identifier(h, Spinel.export(string, enc))
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if escaped.nil?
      escaped.force_encoding(enc)
    end

    def escape_bytea(str)
      Spinel.check_string(str)
      Spinel.conn_escape_bytea(__pg_conn__, str).b
    end

    def unescape_bytea(str)
      Spinel.unescape_bytea(Spinel.c_string(Spinel.check_string(str))).b
    end

    def set_single_row_mode
      h = __pg_conn__
      Spinel.check_frozen(self)
      if Spinel.conn_set_single_row_mode(h) == 0
        Spinel.raise_conn_error(PG::Error, self, "PQsetSingleRowMode #{Spinel.conn_error_message(h)}")
      end
      self
    end

    if Spinel::CHUNK_MODE
      def set_chunked_rows_mode(chunk_size)
        h = __pg_conn__
        Spinel.check_frozen(self)
        if Spinel.conn_set_chunked_rows_mode(h, Spinel.num2int(chunk_size)) == 0
          Spinel.raise_conn_error(PG::Error, self, "PQsetChunkedRowsMode #{Spinel.conn_error_message(h)}")
        end
        self
      end
    end

    def send_query(*args)
      h = __pg_safe__
      if args.length == 1 || (args.length >= 2 && args.length <= 4 && args[1].nil?)
        if Spinel.conn_send_query(h, __pg_cstr__(args[0])) == 0
          Spinel.raise_conn_error(UnableToSend, self, "PQsendQuery #{Spinel.conn_error_message(h)}")
        end
        __pg_wait_for_flush__
        return nil
      end
      Spinel.deprecated(2, "forwarding async_exec to async_exec_params and send_query to send_query_params is deprecated")
      send_query_params(*args)
    end

    def send_query_params(command, params, result_format = nil, typemap = nil)
      h = __pg_safe__
      typemap = __pg_query_typemap__(typemap)
      format = result_format.nil? ? 0 : Spinel.num2int(result_format)
      values, types, formats = __pg_params__(params, true, typemap)
      if Spinel.conn_send_query_params(h, __pg_cstr__(command), values, types, formats, format) == 0
        Spinel.raise_conn_error(UnableToSend, self, "PQsendQueryParams #{Spinel.conn_error_message(h)}")
      end
      __pg_wait_for_flush__
      nil
    end

    def send_prepare(name, command, param_types = nil)
      h = __pg_safe__
      name = __pg_cstr__(name)
      command = __pg_cstr__(command)
      if Spinel.conn_send_prepare(h, name, command, __pg_param_types__(param_types)) == 0
        Spinel.raise_conn_error(UnableToSend, self, "PQsendPrepare #{Spinel.conn_error_message(h)}")
      end
      __pg_wait_for_flush__
      nil
    end

    def send_query_prepared(name, params = nil, result_format = nil, typemap = nil)
      h = __pg_safe__
      params = [] if params.nil?
      typemap = __pg_query_typemap__(typemap)
      format = result_format.nil? ? 0 : Spinel.num2int(result_format)
      values, _, formats = __pg_params__(params, false, typemap)
      if Spinel.conn_send_query_prepared(h, __pg_cstr__(name), values, formats, format) == 0
        Spinel.raise_conn_error(UnableToSend, self, "PQsendQueryPrepared #{Spinel.conn_error_message(h)}")
      end
      __pg_wait_for_flush__
      nil
    end

    # pgconn_send_describe_close_prepared_portal.
    private def __pg_send_name__(name, call, function)
      h = __pg_safe__
      if Spinel.public_send(call, h, name.nil? ? nil : __pg_cstr__(name)) == 0
        Spinel.raise_conn_error(UnableToSend, self, "#{function} #{Spinel.conn_error_message(h)}")
      end
      __pg_wait_for_flush__
      nil
    end

    def send_describe_prepared(stmt_name)
      __pg_send_name__(stmt_name, :conn_send_describe_prepared, "PQsendDescribePrepared")
    end

    def send_describe_portal(portal)
      __pg_send_name__(portal, :conn_send_describe_portal, "PQsendDescribePortal")
    end

    if Spinel::CHUNK_MODE
      def send_close_prepared(stmt_name)
        __pg_send_name__(stmt_name, :conn_send_close_prepared, "PQsendClosePrepared")
      end

      def send_close_portal(portal)
        __pg_send_name__(portal, :conn_send_close_portal, "PQsendClosePortal")
      end
    end

    def sync_get_result(&block)
      handle = Spinel.conn_get_result(__pg_conn__)
      return nil if handle.nil?
      __pg_yield_result__(__pg_result__(handle), &block)
    end

    def consume_input
      h = __pg_conn__
      if Spinel.conn_consume_input(h) == 0
        __pg_close_socket_io__
        Spinel.raise_conn_error(ConnectionBad, self, Spinel.conn_error_message(h))
      end
      nil
    end

    def is_busy
      Spinel.conn_is_busy(__pg_conn__)
    end

    def sync_setnonblocking(state)
      h = __pg_conn__
      Spinel.check_frozen(self)
      arg = case state
            when true then 1
            when false then 0
            else raise ArgumentError, "Boolean value expected"
            end
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_setnonblocking(h, arg) == -1
      nil
    end

    def sync_isnonblocking
      Spinel.conn_isnonblocking(__pg_conn__)
    end

    def sync_flush
      h = __pg_conn__
      ret = Spinel.conn_flush(h)
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if ret == -1
      ret == 0
    end

    # pgconn_async_flush.
    def flush
      while sync_flush == false
        events = socket_io.wait(IO::READABLE | IO::WRITABLE, nil)
        consume_input if events && events & IO::READABLE != 0
      end
      true
    end
    alias_method :async_flush, :flush

    # pgconn_wait_for_flush.
    private def __pg_wait_for_flush__
      __pg_safe__
      return nil unless @__pg_flush_data__
      flush
    end

    private def flush_data=(enabled)
      Spinel.check_frozen(self)
      @__pg_flush_data__ = enabled ? true : false
      enabled
    end

    # wait_socket_readable: until the block answers something, wait for the
    # socket to be readable and consume what came, for at most `timeout`
    # seconds (nil for no limit); nil when the time ran out.
    private def __pg_wait_socket_readable__(timeout)
      h = __pg_conn__
      abort_at = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout if timeout
      until (ret = yield(h))
        wait = abort_at - Process.clock_gettime(Process::CLOCK_MONOTONIC) if timeout
        if !timeout || wait >= 0
          async_flush
          return ret if (ret = yield(h))
          ready = socket_io.wait(IO::READABLE, timeout ? wait : nil)
        else
          ready = false
        end
        return nil unless ready
        if Spinel.conn_consume_input(h) == 0
          __pg_close_socket_io__
          Spinel.raise_conn_error(ConnectionBad, self, "PQconsumeInput() #{Spinel.conn_error_message(h)}")
        end
      end
      ret
    end

    def discard_results
      h = __pg_conn__
      case Spinel.conn_transaction_status(h)
      when PQTRANS_IDLE, PQTRANS_INTRANS, PQTRANS_INERROR
        return nil
      end
      io = socket_io
      loop do
        while Spinel.conn_is_busy(h)
          case Spinel.conn_flush(h)
          when 1
            events = io.wait(IO::READABLE | IO::WRITABLE, nil)
            if events && events & IO::READABLE != 0 && Spinel.conn_consume_input(h) == 0
              return __pg_discard_failed__
            end
          when 0
            io.wait(IO::READABLE, nil)
            return __pg_discard_failed__ if Spinel.conn_consume_input(h) == 0
          else
            return __pg_discard_failed__
          end
        end
        cur = Spinel.conn_get_result(h)
        break if cur.nil?
        status = Spinel.res_status(cur)
        Spinel.res_free(cur)
        if status == PGRES_COPY_IN
          while Spinel.conn_put_copy_end(h, "COPY terminated by new query or discard_results") == 0
            async_flush
          end
        end
        next unless status == PGRES_COPY_OUT
        loop do
          data = Spinel.conn_get_copy_data(h, true)
          if data == 0
            io.wait(IO::READABLE, nil)
            return __pg_discard_failed__ if Spinel.conn_consume_input(h) == 0
          elsif Integer === data
            break
          end
        end
      end
      true
    end

    private def __pg_discard_failed__
      __pg_close_socket_io__
      false
    end

    unless Spinel::CHUNK_MODE
      def sync_cancel
        result = Spinel.conn_cancel(__pg_conn__)
        Spinel.raise_conn_error(PG::Error, self, "Invalid connection!") if result == false
        result&.b
      end
    end

    def notifies
      h = __pg_safe__
      packed = Spinel.conn_notifies(h)
      return nil if packed.nil?
      relname, be_pid, extra = Spinel.unpack(packed)
      enc = __pg_enc__
      {relname: relname.force_encoding(enc), be_pid: be_pid.to_i, extra: extra.force_encoding(enc)}
    end

    def wait_for_notify(timeout = nil)
      __pg_safe__
      timeout = Spinel.num2dbl(timeout) if timeout
      packed = __pg_wait_socket_readable__(timeout) { |h| Spinel.conn_notifies(h) }
      return nil if packed.nil?
      relname, be_pid, extra = Spinel.unpack(packed)
      enc = __pg_enc__
      relname.force_encoding(enc)
      be_pid = be_pid.to_i
      extra = extra.empty? ? nil : extra.force_encoding(enc)
      yield relname, be_pid, extra if block_given?
      relname
    end
    alias_method :notifies_wait, :wait_for_notify

    def sync_put_copy_data(buffer, encoder = nil)
      h = __pg_safe__
      unless encoder.nil? && @__pg_encoder_for_put_copy_data__.nil?
        raise NotImplementedError, "a PG::Coder for put_copy_data is not in pg-spinel yet"
      end
      Spinel.check_string(buffer)
      ret = Spinel.conn_put_copy_data(h, buffer)
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if ret == -1
      ret != 0
    end

    def sync_put_copy_end(*args)
      h = __pg_safe__
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..1)" if args.length > 1
      message = args.empty? ? nil : __pg_cstr__(args[0])
      ret = Spinel.conn_put_copy_end(h, message)
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if ret == -1
      ret != 0
    end

    def sync_get_copy_data(async = nil, decoder = nil)
      h = __pg_safe__
      unless decoder.nil? && @__pg_decoder_for_get_copy_data__.nil?
        raise NotImplementedError, "a PG::Coder for get_copy_data is not in pg-spinel yet"
      end
      data = Spinel.conn_get_copy_data(h, async)
      case data
      when -2 then Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h))
      when -1 then nil
      when 0 then false
      else data.b
      end
    end

    def set_error_verbosity(verbosity)
      h = __pg_conn__
      Spinel.conn_set_error_verbosity(h, Spinel.num2int(verbosity))
    end

    def set_error_context_visibility(context_visibility)
      h = __pg_conn__
      Spinel.conn_set_error_context_visibility(h, Spinel.num2int(context_visibility))
    end

    def trace(stream)
      h = __pg_safe__
      raise ArgumentError, "stream does not respond to method: fileno" unless stream.respond_to?(:fileno)
      fileno = stream.fileno
      raise ArgumentError, "can't get file descriptor from stream" if fileno.nil?
      fd = IO.for_fd(Spinel.num2int(fileno), autoclose: false)
      dup = fd.dup
      dup.autoclose = false
      new_fd = dup.fileno
      raise ArgumentError, "stream is not writable" unless Spinel.conn_trace(h, new_fd)
      @__pg_trace_stream__ = dup
      nil
    end

    def untrace
      h = __pg_safe__
      Spinel.conn_untrace(h)
      @__pg_trace_stream__ = nil
      nil
    end

    def set_notice_receiver
      __pg_safe__
      Spinel.check_frozen(self)
      if block_given?
        raise NotImplementedError, "set_notice_receiver with a block is not in pg-spinel yet: " \
                                   "spinel-ext cannot call into Ruby from libpq"
      end
      old = @__pg_notice_receiver__
      @__pg_notice_receiver__ = nil
      old
    end

    def set_notice_processor
      __pg_safe__
      Spinel.check_frozen(self)
      if block_given?
        raise NotImplementedError, "set_notice_processor with a block is not in pg-spinel yet: " \
                                   "spinel-ext cannot call into Ruby from libpq"
      end
      old = @__pg_notice_processor__
      @__pg_notice_processor__ = nil
      old
    end

    def get_client_encoding
      Spinel.encoding_to_char(Spinel.conn_client_encoding(__pg_conn__)).b
    end

    def sync_set_client_encoding(str)
      h = __pg_conn__
      Spinel.check_frozen(self)
      Spinel.check_string(str)
      if Spinel.conn_set_client_encoding(h, Spinel.c_string(str)) == -1
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h))
      end
      __pg_set_internal_encoding_index__
      nil
    end

    # pgconn_async_set_client_encoding.
    def set_client_encoding(encname)
      Spinel.check_frozen(self)
      Spinel.check_string(encname)
      exec("set client_encoding to #{escape_literal(encname)}")
      __pg_set_internal_encoding_index__
      nil
    end
    alias_method :async_set_client_encoding, :set_client_encoding
    alias_method :client_encoding=, :set_client_encoding

    def block(*args)
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 0..1)" if args.length > 1
      timeout = args.empty? ? nil : Spinel.num2dbl(args[0])
      __pg_wait_socket_readable__(timeout) { |h| !Spinel.conn_is_busy(h) } ? true : false
    end

    def quote_ident(str_or_array)
      Spinel.quote_identifier(str_or_array, __pg_enc__)
    end

    def sync_get_last_result
      h = __pg_conn__
      prev = nil
      while (cur = Spinel.conn_get_result(h))
        Spinel.res_free(prev) if prev
        prev = cur
        status = Spinel.res_status(cur)
        break if status == PGRES_COPY_OUT || status == PGRES_COPY_IN || status == PGRES_COPY_BOTH
      end
      prev && __pg_checked_result__(prev)
    end

    # pgconn_async_get_last_result.
    private def __pg_async_get_last_result__
      h = __pg_conn__
      prev = nil
      loop do
        __pg_wait_socket_readable__(nil) { |handle| !Spinel.conn_is_busy(handle) }
        cur = Spinel.conn_get_result(h)
        break if cur.nil?
        Spinel.res_free(prev) if prev
        prev = cur
        status = Spinel.res_status(cur)
        break if status == PGRES_COPY_OUT || status == PGRES_COPY_IN || status == PGRES_COPY_BOTH
      end
      prev && __pg_checked_result__(prev)
    end

    def get_last_result
      __pg_async_get_last_result__
    end
    alias_method :async_get_last_result, :get_last_result

    def sync_encrypt_password(password, username, algorithm = nil)
      h = __pg_conn__
      Spinel.check_string(password)
      Spinel.check_string(username)
      algorithm = algorithm ? Spinel.c_string(algorithm) : nil
      encrypted = Spinel.conn_encrypt_password(h, Spinel.c_string(password), Spinel.c_string(username), algorithm)
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if encrypted.nil?
      encrypted.b
    end

    def ssl_in_use?
      Spinel.conn_ssl_in_use(__pg_conn__)
    end

    def ssl_attribute(attribute_name)
      h = __pg_conn__
      Spinel.conn_ssl_attribute(h, Spinel.c_string(attribute_name))&.force_encoding(Encoding::UTF_8)
    end

    def ssl_attribute_names
      Spinel.unpack(Spinel.conn_ssl_attribute_names(__pg_conn__)).map { |n| n.force_encoding(Encoding::UTF_8) }
    end

    if Spinel::PIPELINING
      def pipeline_status
        Spinel.conn_pipeline_status(__pg_conn__)
      end

      def enter_pipeline_mode
        h = __pg_conn__
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_enter_pipeline_mode(h) != 1
        nil
      end

      def exit_pipeline_mode
        h = __pg_conn__
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_exit_pipeline_mode(h) != 1
        nil
      end

      def sync_pipeline_sync
        h = __pg_conn__
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_pipeline_sync(h) != 1
        nil
      end

      if Spinel::CHUNK_MODE
        def send_pipeline_sync
          h = __pg_conn__
          Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_send_pipeline_sync(h) != 1
          nil
        end
      end

      def send_flush_request
        h = __pg_conn__
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if Spinel.conn_send_flush_request(h) != 1
        nil
      end
    end

    def lo_creat(mode = nil)
      h = __pg_conn__
      mode = mode.nil? ? INV_READ : Spinel.num2int(mode)
      oid = Spinel.conn_lo_creat(h, mode)
      Spinel.raise_conn_error(PG::Error, self, "lo_creat failed") if oid == 0
      oid
    end
    alias_method :locreat, :lo_creat

    def lo_create(in_lo_oid)
      h = __pg_conn__
      oid = Spinel.conn_lo_create(h, Spinel.num2uint(in_lo_oid))
      Spinel.raise_conn_error(PG::Error, self, "lo_create failed") if oid == 0
      oid
    end
    alias_method :locreate, :lo_create

    def lo_import(filename)
      h = __pg_conn__
      Spinel.check_string(filename)
      oid = Spinel.conn_lo_import(h, Spinel.c_string(filename))
      Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h)) if oid == 0
      oid
    end
    alias_method :loimport, :lo_import

    def lo_export(lo_oid, filename)
      h = __pg_conn__
      oid = Spinel.num2uint(lo_oid)
      Spinel.check_string(filename)
      if Spinel.conn_lo_export(h, oid, Spinel.c_string(filename)) < 0
        Spinel.raise_conn_error(PG::Error, self, Spinel.conn_error_message(h))
      end
      nil
    end
    alias_method :loexport, :lo_export

    def lo_open(lo_oid, mode = nil)
      h = __pg_conn__
      oid = Spinel.num2uint(lo_oid)
      mode = mode.nil? ? INV_READ : Spinel.num2int(mode)
      fd = Spinel.conn_lo_open(h, oid, mode)
      Spinel.raise_conn_error(PG::Error, self, "can't open large object: #{Spinel.conn_error_message(h)}") if fd < 0
      fd
    end
    alias_method :loopen, :lo_open

    def lo_write(lo_desc, buffer)
      h = __pg_conn__
      fd = Spinel.num2int(lo_desc)
      Spinel.check_string(buffer)
      n = Spinel.conn_lo_write(h, fd, buffer)
      Spinel.raise_conn_error(PG::Error, self, "lo_write failed: #{Spinel.conn_error_message(h)}") if n < 0
      n
    end
    alias_method :lowrite, :lo_write

    def lo_read(lo_desc, len)
      h = __pg_conn__
      len = Spinel.num2int(len)
      fd = Spinel.num2int(lo_desc)
      Spinel.raise_conn_error(PG::Error, self, "negative length #{len} given") if len < 0
      data = Spinel.conn_lo_read(h, fd, len)
      Spinel.raise_conn_error(PG::Error, self, "lo_read failed") if Integer === data && data < 0
      return nil if data == 0
      data.b
    end
    alias_method :loread, :lo_read

    def lo_lseek(lo_desc, offset, whence)
      h = __pg_conn__
      fd = Spinel.num2int(lo_desc)
      ret = Spinel.conn_lo_lseek(h, fd, Spinel.num2int(offset), Spinel.num2int(whence))
      Spinel.raise_conn_error(PG::Error, self, "lo_lseek failed") if ret < 0
      ret
    end
    alias_method :lolseek, :lo_lseek
    alias_method :lo_seek, :lo_lseek
    alias_method :loseek, :lo_lseek

    def lo_tell(lo_desc)
      h = __pg_conn__
      position = Spinel.conn_lo_tell(h, Spinel.num2int(lo_desc))
      Spinel.raise_conn_error(PG::Error, self, "lo_tell failed") if position < 0
      position
    end
    alias_method :lotell, :lo_tell

    def lo_truncate(lo_desc, len)
      h = __pg_conn__
      fd = Spinel.num2int(lo_desc)
      ret = Spinel.conn_lo_truncate(h, fd, Spinel.num2int(len))
      Spinel.raise_conn_error(PG::Error, self, "lo_truncate failed") if ret < 0
      nil
    end
    alias_method :lotruncate, :lo_truncate

    def lo_close(lo_desc)
      h = __pg_conn__
      ret = Spinel.conn_lo_close(h, Spinel.num2int(lo_desc))
      Spinel.raise_conn_error(PG::Error, self, "lo_close failed") if ret < 0
      nil
    end
    alias_method :loclose, :lo_close

    def lo_unlink(oid)
      h = __pg_conn__
      ret = Spinel.conn_lo_unlink(h, Spinel.num2uint(oid))
      Spinel.raise_conn_error(PG::Error, self, "lo_unlink failed") if ret < 0
      nil
    end
    alias_method :lounlink, :lo_unlink

    # pgconn_set_internal_encoding_index.
    private def __pg_set_internal_encoding_index__
      h = __pg_safe__
      @__pg_enc__ = Spinel.rb_encoding(Spinel.encoding_to_char(Spinel.conn_client_encoding(h)))
    end

    def internal_encoding
      h = __pg_conn__
      Spinel.rb_encoding(Spinel.encoding_to_char(Spinel.conn_client_encoding(h)))
    end

    def internal_encoding=(enc)
      Spinel.check_frozen(self)
      if enc.nil?
        sync_set_client_encoding("SQL_ASCII")
        return enc
      elsif String === enc && enc.casecmp?("JOHAB")
        sync_set_client_encoding("JOHAB")
        return enc
      end
      rbenc = Encoding === enc ? enc : Encoding.find(Spinel.string_value(enc))
      name = Spinel.pg_encoding(rbenc)
      if Spinel.conn_set_client_encoding(__pg_conn__, name) == -1
        server_encoding = external_encoding
        raise Encoding::CompatibilityError, "incompatible character encodings: #{server_encoding.name} and #{name}"
      end
      __pg_set_internal_encoding_index__
      enc
    end

    def external_encoding
      h = __pg_safe__
      Spinel.rb_encoding(Spinel.conn_parameter_status(h, "server_encoding").to_s)
    end

    def set_default_encoding
      h = __pg_conn__
      Spinel.check_frozen(self)
      if (enc = Encoding.default_internal)
        if Spinel.rb_encoding(Spinel.encoding_to_char(Spinel.conn_client_encoding(h))) != enc
          encname = Spinel.pg_encoding(enc)
          begin
            set_client_encoding(encname)
          rescue StandardError
            if $VERBOSE
              warn("Failed to set the default_internal encoding to #{encname}: '#{Spinel.conn_error_message(h)}'")
            end
          end
        end
        __pg_set_internal_encoding_index__
        enc
      else
        __pg_set_internal_encoding_index__
        nil
      end
    end

    def type_map_for_queries=(typemap)
      Spinel.check_frozen(self)
      @__pg_type_map_for_queries__ = Spinel.check_type_map(typemap)
    end

    def type_map_for_queries
      @__pg_type_map_for_queries__ || Spinel::ALL_STRINGS
    end

    def type_map_for_results=(typemap)
      Spinel.check_frozen(self)
      @__pg_type_map_for_results__ = Spinel.check_type_map(typemap)
    end

    def type_map_for_results
      @__pg_type_map_for_results__ || Spinel::ALL_STRINGS
    end

    def encoder_for_put_copy_data=(encoder)
      Spinel.check_frozen(self)
      unless encoder.nil? || Coder === encoder
        raise TypeError, "wrong argument type #{Spinel.type_name(encoder)} (expected PG::Coder)"
      end
      @__pg_encoder_for_put_copy_data__ = encoder
    end

    def encoder_for_put_copy_data
      @__pg_encoder_for_put_copy_data__
    end

    def decoder_for_get_copy_data=(decoder)
      Spinel.check_frozen(self)
      unless decoder.nil? || Coder === decoder
        raise TypeError, "wrong argument type #{Spinel.type_name(decoder)} (expected PG::Coder)"
      end
      @__pg_decoder_for_get_copy_data__ = decoder
    end

    def decoder_for_get_copy_data
      @__pg_decoder_for_get_copy_data__
    end

    def field_name_type=(sym)
      Spinel.check_frozen(self)
      unless sym == :symbol || sym == :string
        raise ArgumentError, "invalid argument #{sym.inspect}"
      end
      @__pg_field_names_symbol__ = sym == :symbol
      sym
    end

    def field_name_type
      @__pg_field_names_symbol__ ? :symbol : :string
    end
  end

  module Spinel
    # pg_text_enc_identifier: `value`, a String or an Array of them (a
    # schema-qualified name), as a quoted identifier in `enc`.
    def self.quote_identifier(value, enc)
      out = if Array === value
        value.map { |part| quote_identifier_part(string_value(part), enc) }.join(".".b)
      else
        quote_identifier_part(string_value(value), enc)
      end
      out.force_encoding(enc)
    end

    # quote_identifier.
    def self.quote_identifier_part(str, enc)
      bytes = export(str, enc).b
      raise ArgumentError, "string contains null byte" if bytes.include?("\0")
      "\"".b + bytes.gsub("\"".b, "\"\"".b) + "\"".b
    end
  end
end
