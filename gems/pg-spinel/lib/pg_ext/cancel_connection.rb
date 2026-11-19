# frozen_string_literal: true

# pg_cancel_connection.c: PG::CancelConnection, libpq 17's cancel
# connection, which upstream's `lib/pg/cancel_connection.rb` and
# `PG::Connection#cancel` build on. Defined only when the loaded libpq has
# it, as upstream compiles it in only for one that does. Its
# `PGcancelConn` is a box held by handle in `@__pg_cancon__`.

module PG
  if Spinel::CHUNK_MODE
    class CancelConnection
      include Enumerable

      def self.allocate
        cancon = super
        cancon.instance_variable_set(:@__pg_cancon__, nil)
        cancon
      end

      private def initialize_copy(original)
        super
        ObjectSpace.undefine_finalizer(self)
        @__pg_cancon__ = nil
        @__pg_socket_io__ = nil
        self
      end

      def initialize(conn)
        h = conn.__send__(:__pg_conn__)
        handle = Spinel.cancon_new(h)
        Spinel.raise_conn_error(ConnectionBad, self, "PQcancelCreate failed") if handle.nil?
        @__pg_cancon__ = handle
        ObjectSpace.define_finalizer(self, Spinel.cancon_finalizer(handle))
        self
      end

      private def __pg_cancon__
        h = @__pg_cancon__
        unless h && Spinel.cancon_open?(h)
          Spinel.raise_conn_error(ConnectionBad, self, "PG::CancelConnection is closed")
        end
        h
      end

      private def __pg_close_socket_io__
        if (io = @__pg_socket_io__)
          io.close
        end
        @__pg_socket_io__ = nil
      end

      def sync_cancel
        h = __pg_cancon__
        __pg_close_socket_io__
        if Spinel.cancon_blocking(h) == 0
          Spinel.raise_conn_error(ConnectionBad, self, "PQcancelBlocking #{Spinel.cancon_error_message(h)}")
        end
        nil
      end

      def start
        h = __pg_cancon__
        __pg_close_socket_io__
        if Spinel.cancon_start(h) == 0
          Spinel.raise_conn_error(ConnectionBad, self, "PQcancelStart #{Spinel.cancon_error_message(h)}")
        end
        nil
      end

      def error_message
        Spinel.cancon_error_message(__pg_cancon__)&.b
      end

      def poll
        h = __pg_cancon__
        __pg_close_socket_io__
        Spinel.cancon_poll(h)
      end

      def status
        Spinel.cancon_status(__pg_cancon__)
      end

      def socket_io
        return @__pg_socket_io__ if @__pg_socket_io__
        sd = Spinel.cancon_socket(@__pg_cancon__)
        Spinel.raise_conn_error(ConnectionBad, self, "PQcancelSocket() can't get socket descriptor") if sd < 0
        io = BasicSocket.for_fd(sd)
        io.autoclose = false
        @__pg_socket_io__ = io
      end

      def reset
        h = __pg_cancon__
        __pg_close_socket_io__
        Spinel.cancon_reset(h)
        nil
      end

      def finish
        __pg_close_socket_io__
        Spinel.cancon_finish(@__pg_cancon__) if @__pg_cancon__
        nil
      end
    end
  end
end
