# frozen_string_literal: true

# The sqlite3 gem's C extension (`ext/sqlite3/*.c` at 2.10.2), in Ruby, over
# the SQLite calls of `SQLite3::Spinel`, the gem's `.spinel` extension
# (`ext/sqlite3_spinel`). `require "sqlite3"` loads this file where CRuby
# loads `sqlite3_native.so`; the rest of `lib/` is upstream's, unchanged.
#
# Each method does what its C function does: the same checks in the same
# order, the same exception classes and messages, and the same values, with
# the same encodings and frozenness. A connection's and a statement's
# SQLite state are boxes the extension owns, held here by handle in
# instance variables spelled `@__name__`, which `instance_variables` leaves
# out as it leaves out a typed data struct, and freed by a finalizer.
#
# ponytail: what calls back into Ruby from inside SQLite is not here, because
# `spinel-ext` cannot call a Ruby method: `create_function`,
# `create_aggregate`, `define_aggregator`, `collation`, `trace`, a block
# given to `busy_handler` (so `busy_handler_timeout=`), `authorizer=` with
# an authorizer, and `statement_timeout=`, each a NotImplementedError that
# says so; nor `SQLite3::Backup`, `serialize` and `deserialize`. Upgrade: a
# call-a-method entry in `spinel-ext`'s API, then each in the C extension's
# shape.

unless RUBY_ENGINE == "spinel"
  raise LoadError, "sqlite3-spinel is the sqlite3 gem for Spinel and loads only there; " \
                   "on #{RUBY_ENGINE}, install the sqlite3 gem"
end

require "sqlite3/sqlite3_spinel"

module SQLite3
  # sqlite3.c: a class for telling blobs from strings when binding them.
  class Blob < String; end

  module Constants
    # sqlite3.c: the `sqlite3_open_v2` flags, in its order.
    module Open
      READONLY = 0x00000001
      READWRITE = 0x00000002
      CREATE = 0x00000004
      DELETEONCLOSE = 0x00000008
      EXCLUSIVE = 0x00000010
      MAIN_DB = 0x00000100
      TEMP_DB = 0x00000200
      TRANSIENT_DB = 0x00000400
      MAIN_JOURNAL = 0x00000800
      TEMP_JOURNAL = 0x00001000
      SUBJOURNAL = 0x00002000
      MASTER_JOURNAL = 0x00004000
      SUPER_JOURNAL = 0x00004000
      NOMUTEX = 0x00008000
      FULLMUTEX = 0x00010000
      AUTOPROXY = 0x00000020
      SHAREDCACHE = 0x00020000
      PRIVATECACHE = 0x00040000
      WAL = 0x00080000
      URI = 0x00000040
      MEMORY = 0x00000080
    end
  end

  # The library is the amalgamation compiled into the extension, so the
  # version it was compiled with is the one loaded.
  SQLITE_VERSION = Spinel.libversion
  SQLITE_VERSION_NUMBER = Spinel.libversion_number
  SQLITE_LOADED_VERSION = Spinel.libversion
  SQLITE_PACKAGED_LIBRARIES = true
  SQLITE_PRECOMPILED_LIBRARIES = true

  class << self
    def sqlcipher?
      false
    end

    def libversion
      Spinel.libversion_number
    end

    def threadsafe
      Spinel.threadsafe
    end

    def status(parameter, reset_flag = false)
      current, highwater = Spinel.status(Spinel.num2int(parameter), reset_flag).unpack("q<q<")
      {current: current, highwater: highwater}
    end
  end

  # Ruby helpers beside the extension's SQLite calls.
  module Spinel
    # exception.c's status2klass: the class for a result code's low 8 bits.
    EXCEPTIONS = %w[
      nil SQLException InternalException PermissionException AbortException
      BusyException LockedException MemoryException ReadOnlyException
      InterruptException IOException CorruptException NotFoundException
      FullException CantOpenException ProtocolException EmptyException
      SchemaChangedException TooBigException ConstraintException
      MismatchException MisuseException UnsupportedException
      AuthorizationException FormatException RangeException
      NotADatabaseException
    ].freeze

    ROW = 100
    DONE = 101

    INT64 = (-2**63)..(2**63 - 1)

    class << self
      # `NUM2INT`: an Integer, or a Float truncated, in a C int.
      def num2int(value)
        int = Integer === value ? value : Integer(value.to_int)
        raise RangeError, "integer #{int} too big to convert to 'int'" unless (-2**31...2**31).cover?(int)
        int
      rescue NoMethodError
        raise TypeError, "no implicit conversion from nil to integer" if value.nil?
        raise TypeError, "no implicit conversion of #{value.class} into Integer"
      end

      # `StringValue`.
      def string_value(value)
        return value if String === value
        return value.to_str if value.respond_to?(:to_str)
        raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into String"
      end

      # `StringValueCStr`: a String with no NUL in it.
      def c_string(value)
        value = string_value(value)
        raise ArgumentError, "string contains null byte" if value.include?("\0")
        value
      end

      # A C string's bytes as `rb_str_new2` makes them: up to the first NUL.
      def c_bytes(value)
        value.b.split("\0", 2).first || "".b
      end

      def exception_class(status)
        name = EXCEPTIONS[status & 0xff] || "Exception"
        name == "nil" ? nil : SQLite3.const_get(name)
      end

      # exception.c's rb_sqlite3_raise_msg: raise for a failed `status` with
      # `message`, its `code` set; nothing for SQLITE_OK.
      def raise_status(status, message, sql: nil, offset: -1)
        klass = exception_class(status) or return
        exception = klass.new(message)
        exception.instance_variable_set(:@code, status)
        if sql
          exception.instance_variable_set(:@sql, sql)
          exception.instance_variable_set(:@sql_offset, offset)
        end
        raise exception
      end

      # exception.c's rb_sqlite3_raise: `errmsg` is the message, read as the
      # C string `rb_exc_new2` copies.
      def check(db, status)
        return if status & 0xff == 0
        raise_status(status, db_errmsg(db))
      end

      def db_finalizer(state)
        proc { release(state) }
      end

      def stmt_finalizer(handle)
        proc { stmt_free(handle) }
      end

      # database.c's close_or_discard_db: a connection carried across a
      # fork that can write is discarded rather than closed. `state` is
      # `[handle, owner pid, readonly]`.
      # Answers whether it discarded.
      def close_or_discard(state)
        handle, owner, readonly = state
        if readonly || owner == Process.pid
          db_close(handle)
          false
        else
          db_discard(handle)
          true
        end
      end

      def release(state)
        close_or_discard(state)
        db_free(state[0])
      end

      # The handle of `database`'s connection, as `TypedData_Get_Struct`
      # finds its struct.
      def db_of(database)
        unless SQLite3::Database === database
          raise TypeError, "wrong argument type #{database.class} (expected SQLite3::Database)"
        end
        database.instance_variable_get(:@__sqlite3_state__)&.first
      end
    end
  end
  private_constant :Spinel

  class Database
    def collation(name, comparator)
      not_on_spinel("SQLite3::Database#collation")
    end

    def close
      state = @__sqlite3_state__
      if state && Spinel.db_open?(state[0]) && Spinel.close_or_discard(state)
        @__sqlite3_discarded__ = true
      end
      self
    end

    def closed?
      !(state = @__sqlite3_state__) || !Spinel.db_open?(state[0])
    end

    def total_changes
      Spinel.db_total_changes(open_db)
    end

    def trace(*)
      not_on_spinel("SQLite3::Database#trace")
    end

    def last_insert_row_id
      Spinel.db_last_insert_rowid(open_db)
    end

    def define_function(name)
      not_on_spinel("SQLite3::Database#define_function")
    end

    def define_function_with_flags(name, flags)
      not_on_spinel("SQLite3::Database#define_function_with_flags")
    end

    def interrupt
      Spinel.db_interrupt(open_db)
      self
    end

    # `rb_str_new2`: the message as bytes.
    def errmsg
      Spinel.c_bytes(Spinel.db_errmsg(open_db))
    end

    def errcode
      Spinel.db_errcode(open_db)
    end

    def complete?(sql)
      Spinel.complete(Spinel.c_bytes(Spinel.string_value(sql)))
    end

    def changes
      Spinel.db_changes(open_db)
    end

    def authorizer=(authorizer)
      open_db
      not_on_spinel("SQLite3::Database#authorizer=") unless authorizer.nil?
      self
    end

    def busy_handler(*args, &block)
      db = open_db
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
      handler = args.first
      handler = block if handler.nil? && block
      not_on_spinel("SQLite3::Database#busy_handler with a handler") unless handler.nil?
      # No handler is SQLite's own default, which a zero busy timeout puts
      # back: both are the one busy handler slot.
      Spinel.check(db, Spinel.db_busy_timeout(db, 0))
      self
    end

    def busy_timeout=(timeout)
      db = open_db
      Spinel.check(db, Spinel.db_busy_timeout(db, Spinel.num2int(timeout)))
      self
    end

    def statement_timeout=(timeout)
      not_on_spinel("SQLite3::Database#statement_timeout=")
    end

    def extended_result_codes=(enable)
      db = open_db
      Spinel.check(db, Spinel.db_extended_result_codes(db, enable))
      self
    end

    def transaction_active?
      !Spinel.db_autocommit(open_db)
    end

    def enable_load_extension(onoff)
      db = open_db
      flag = case onoff
             when true then 1
             when false then 0
             else Spinel.num2int(onoff)
             end
      Spinel.check(db, Spinel.db_enable_load_extension(db, flag))
      self
    end

    private

    def open_v2(file, mode, zvfs)
      if (state = @__sqlite3_state__) && Spinel.db_open?(state[0])
        raise SQLite3::Exception, "cannot open a database that is already open"
      end
      flags = Spinel.num2int(mode)
      file = Spinel.c_string(file)
      zvfs = Spinel.c_string(zvfs) unless zvfs.nil?
      state = connection_state
      status = Spinel.db_open(state[0], file, flags, zvfs)
      if status != 0
        message = Spinel.db_errmsg(state[0])
        Spinel.db_close(state[0])
        Spinel.raise_status(status, message)
      end
      state[2] = flags & Constants::Open::READONLY != 0
      self
    end

    # The UTF-16 open, as `sqlite3_open16` does it: read-write, created if
    # missing, and a new database's text in UTF-16.
    #
    # ponytail: the filename is converted to UTF-8 and opened with
    # `sqlite3_open_v2`, then `PRAGMA encoding` makes a new database UTF-16,
    # which is what `sqlite3_open16` differs by. Upgrade: `sqlite3_open16`
    # in the extension.
    def open16(file)
      file = Spinel.string_value(file)
      utf16 = file.dup
      unless utf16.encoding == ::Encoding::UTF_16LE || utf16.encoding == ::Encoding::UTF_16BE
        native = [1].pack("S").bytes.first == 1 ? ::Encoding::UTF_16LE : ::Encoding::UTF_16BE
        utf16.force_encoding(native)
      end
      raise ArgumentError, "string contains null char" if utf16.codepoints.include?(0)
      open_v2(utf16.encode(::Encoding::UTF_8), Constants::Open::READWRITE | Constants::Open::CREATE, nil)
      Spinel.check(open_db, run_pragma_utf16)
      self
    end

    def run_pragma_utf16
      statement = Spinel.stmt_new
      status = Spinel.stmt_prepare(statement, open_db, "PRAGMA encoding = 'UTF-16'")
      status = Spinel.stmt_step(statement) if status == 0
      status == Spinel::DONE ? 0 : status
    ensure
      Spinel.stmt_free(statement)
    end

    def discard
      state = connection_state
      Spinel.db_discard(state[0])
      @__sqlite3_discarded__ = true
      self
    end

    def define_aggregator2(aggregator, name)
      not_on_spinel("SQLite3::Database#create_aggregate")
    end

    def disable_quirk_mode
      state = @__sqlite3_state__
      return false unless state && Spinel.db_open?(state[0])
      Spinel.db_disable_dqs(state[0])
    end

    # database.c's exec_batch, which hands `sqlite3_exec` a callback that
    # keeps each row as its columns' text, nil for NULL: the same loop over
    # the statements in `sql`, each row read as `sqlite3_exec` reads it.
    def exec_batch(sql, results_as_hash)
      db = open_db
      rest = Spinel.c_bytes(Spinel.string_value(sql))
      rows = []
      statement = Spinel.stmt_new
      begin
        until rest.empty?
          status = Spinel.stmt_prepare(statement, db, rest)
          if status != 0
            Spinel.raise_status(status, Spinel.c_bytes(Spinel.db_errmsg(db)))
          end
          rest = rest.byteslice(Spinel.stmt_tail(statement)..)
          next unless Spinel.stmt_open?(statement)
          names = nil
          while (status = Spinel.stmt_step(statement)) == Spinel::ROW
            count = Spinel.stmt_column_count(statement)
            names ||= Array.new(count) { |i| Spinel.c_bytes(Spinel.stmt_column_name(statement, i)) }
            values = Array.new(count) do |i|
              Spinel.stmt_column_type(statement, i) == 5 ? nil : Spinel.c_bytes(Spinel.stmt_column_text(statement, i))
            end
            rows << (results_as_hash == true ? names.zip(values).to_h : values)
          end
          if status != Spinel::DONE
            message = Spinel.c_bytes(Spinel.db_errmsg(db))
            Spinel.stmt_finalize(statement)
            Spinel.raise_status(status, message)
          end
          Spinel.stmt_finalize(statement)
        end
      ensure
        Spinel.stmt_free(statement)
      end
      rows
    end

    def db_filename(db_name)
      name = Spinel.db_filename(open_db, Spinel.c_string(db_name))
      name&.force_encoding(::Encoding::UTF_8)
    end

    def load_extension_internal(file)
      db = open_db
      status = Spinel.db_load_extension(db, Spinel.c_bytes(Spinel.string_value(file)))
      message = Spinel.db_message(db)
      Spinel.raise_status(status, message) if status & 0xff != 0
      self
    end

    # This connection's state, `[handle, owner pid, readonly]`, made on the
    # first open with the finalizer that frees it.
    def connection_state
      @__sqlite3_state__ ||= begin
        state = [Spinel.db_new, Process.pid, false]
        ObjectSpace.define_finalizer(self, Spinel.db_finalizer(state))
        state
      end
    end

    # REQUIRE_OPEN_DB.
    def open_db
      state = @__sqlite3_state__
      unless state && Spinel.db_open?(state[0])
        raise SQLite3::Exception, "cannot use a closed database"
      end
      state[0]
    end

    def not_on_spinel(what)
      raise NotImplementedError, "#{what} is not supported by sqlite3-spinel: " \
                                 "it would call into Ruby from SQLite, which Spinel's extension API cannot yet do"
    end
  end

  class Statement
    # stmt_stat_internal's keys, with their SQLITE_STMTSTATUS_ values.
    STATS = {
      fullscan_steps: 1,
      sorts: 2,
      autoindexes: 3,
      vm_steps: 4,
      reprepares: 5,
      runs: 6,
      filter_misses: 7,
      filter_hits: 8
    }.freeze
    private_constant :STATS

    def close
      handle = open_stmt
      Spinel.stmt_finalize(handle)
      self
    end

    def closed?
      !(handle = @__sqlite3_stmt__) || !Spinel.stmt_open?(handle)
    end

    def bind_param(key, value)
      handle = live_stmt
      index = case key
              when Symbol, String
                key = key.to_s
                key = ":#{key}" unless key.start_with?(":")
                Spinel.stmt_bind_parameter_index(handle, key)
              else
                Spinel.num2int(key)
              end
      raise SQLite3::Exception, "no such bind parameter" if index == 0

      status = case value
               when String
                 if value.class == Blob || value.encoding == ::Encoding::ASCII_8BIT
                   Spinel.stmt_bind_blob(handle, index, value)
                 elsif value.encoding == ::Encoding::UTF_16LE || value.encoding == ::Encoding::UTF_16BE
                   Spinel.stmt_bind_text16(handle, index, value)
                 else
                   Spinel.stmt_bind_text(handle, index, value.encode(::Encoding::UTF_8))
                 end
               when Integer
                 if Spinel::INT64.cover?(value)
                   Spinel.stmt_bind_int64(handle, index, value)
                 else
                   Spinel.stmt_bind_double(handle, index, [value.to_f].pack("E"))
                 end
               when Float
                 Spinel.stmt_bind_double(handle, index, [value].pack("E"))
               when nil
                 Spinel.stmt_bind_null(handle, index)
               else
                 raise RuntimeError, "can't prepare #{value.class}"
               end
      Spinel.check(db_handle, status)
      self
    end

    def reset!
      Spinel.stmt_reset(live_stmt)
      @__sqlite3_done__ = false
      self
    end

    def clear_bindings!
      Spinel.stmt_clear_bindings(live_stmt)
      @__sqlite3_done__ = false
      self
    end

    def step
      handle = live_stmt
      return nil if @__sqlite3_done__

      internal = ::Encoding.default_internal
      value = Spinel.stmt_step(handle)
      if value == Spinel::DONE
        @__sqlite3_done__ = true
        return nil
      end

      length = Spinel.stmt_column_count(handle)
      list = []
      if value == Spinel::ROW
        length.times do |i|
          list << case Spinel.stmt_column_type(handle, i)
                  when 1 then Spinel.stmt_column_int64(handle, i)
                  when 2 then Spinel.stmt_column_double(handle, i).unpack1("E")
                  when 3
                    text = Spinel.stmt_column_text(handle, i)
                    text = text.encode(internal) if internal
                    text.freeze
                  when 4 then Spinel.stmt_column_blob(handle, i).b.freeze
                  when 5 then nil
                  else raise RuntimeError, "bad type"
                  end
        end
      else
        Spinel.stmt_reset(handle)
        @__sqlite3_done__ = false
        Spinel.check(db_handle, value)
      end
      list.freeze
    end

    def done?
      @__sqlite3_done__ ? true : false
    end

    def column_count
      Spinel.stmt_column_count(live_stmt)
    end

    def column_name(index)
      name = Spinel.stmt_column_name(live_stmt, Spinel.num2int(index))
      name && -name
    end

    def column_decltype(index)
      Spinel.stmt_column_decltype(live_stmt, Spinel.num2int(index))
    end

    def bind_parameter_count
      Spinel.stmt_bind_parameter_count(live_stmt)
    end

    def named_params
      handle = live_stmt
      params = []
      (1..Spinel.stmt_bind_parameter_count(handle)).each do |i|
        name = Spinel.stmt_bind_parameter_name(handle, i)
        params << -name.byteslice(1..) if name && !name.start_with?("?")
      end
      params.freeze
    end

    def sql
      Spinel.stmt_sql(live_stmt).freeze
    end

    def expanded_sql
      Spinel.stmt_expanded_sql(live_stmt).freeze
    end

    def database_name(index)
      Spinel.stmt_column_database_name(live_stmt, Spinel.num2int(index))
    end

    def memused
      Spinel.stmt_status(live_stmt, 99, false)
    end

    private

    def column_decltype_downcased(index)
      name = Spinel.stmt_column_decltype(live_stmt, Spinel.num2int(index))
      name && -name.downcase(:ascii)
    end

    def prepare(db, sql)
      sql = Spinel.string_value(sql)
      database = Spinel.db_of(db)
      @__sqlite3_database__ = db
      handle = (@__sqlite3_stmt__ ||= Spinel.stmt_new.tap do |h|
        ObjectSpace.define_finalizer(self, Spinel.stmt_finalizer(h))
      end)
      unless database && Spinel.db_open?(database)
        raise SQLite3::Exception, "cannot use a closed database"
      end
      status = Spinel.stmt_prepare(handle, database, sql)
      if status & 0xff != 0
        Spinel.raise_status(status, Spinel.c_bytes(Spinel.db_errmsg(database)),
          sql: Spinel.c_bytes(sql), offset: Spinel.db_error_offset(database))
      end
      tail = sql.byteslice(Spinel.stmt_tail(handle)..)
      Spinel.c_bytes(tail).force_encoding(::Encoding::UTF_8)
    end

    def stats_as_hash
      handle = live_stmt
      STATS.to_h { |key, op| [key, Spinel.stmt_status(handle, op, false)] }
    end

    def stat_for(key)
      handle = live_stmt
      raise TypeError, "non-symbol given" unless Symbol === key
      op = STATS.fetch(key) { raise ArgumentError, "unknown key: #{key}" }
      Spinel.stmt_status(handle, op, false)
    end

    # REQUIRE_OPEN_STMT.
    def open_stmt
      handle = @__sqlite3_stmt__
      unless handle && Spinel.stmt_open?(handle)
        raise SQLite3::Exception, "cannot use a closed statement"
      end
      handle
    end

    # REQUIRE_LIVE_DB, then REQUIRE_OPEN_STMT.
    def live_stmt
      database = @__sqlite3_database__
      if database&.instance_variable_get(:@__sqlite3_discarded__)
        raise SQLite3::Exception, "cannot use a statement associated with a discarded database"
      end
      open_stmt
    end

    def db_handle
      Spinel.db_of(@__sqlite3_database__)
    end
  end
end
