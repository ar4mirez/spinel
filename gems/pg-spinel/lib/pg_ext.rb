# frozen_string_literal: true

# The pg gem's C extension (`ext/*.c` at 1.7.0), in Ruby, over the libpq
# calls of `PG::Spinel`, the gem's `.spinel` extension (`ext/pg_spinel`).
# Upstream's `lib/pg.rb` requires `pg_ext` where CRuby loads `pg_ext.so`, and
# finds this file; the rest of `lib/` is upstream's, unchanged.
#
# Each method does what its C function does: the same checks in the same
# order, the same exception classes and messages, and the same values, with
# the same encodings and frozenness. A connection's, a result's and a cancel
# connection's libpq state are boxes the extension owns, held here by handle
# in instance variables spelled `@__name__`, which `instance_variables`
# leaves out as it leaves out a typed data struct, and freed by a finalizer.
#
# The extension is linked with the system's libpq, as upstream's source gem
# is. What upstream compiles in only for a newer libpq (`hostaddr`, pipeline
# mode, chunked rows, `close_prepared`, `PG::CancelConnection`) is defined
# here only when the loaded libpq has it, which `PG::Spinel.has` says.
#
# ponytail: values come back as Strings, as upstream's do under its default
# `PG::TypeMapAllStrings`; the other type maps and the coders, which turn
# them into Integers, Times and the rest, are not here yet: their classes
# exist, and making one raises NotImplementedError. Nor is what calls back
# into Ruby from inside libpq, as `spinel-ext` cannot call a Ruby method: a
# block given to `set_notice_receiver` or `set_notice_processor` raises
# NotImplementedError. Upgrade: the type maps and coders in Ruby over the
# Strings, which needs nothing of the extension; then a call-a-method entry in
# `spinel-ext`'s API for the notice callbacks.

unless RUBY_ENGINE == "spinel"
  raise LoadError, "pg-spinel is the pg gem for Spinel and loads only there; " \
                   "on #{RUBY_ENGINE}, install the pg gem"
end

require "pg_ext/pg_spinel"

module PG
  module Spinel
    # libpq's major version, which decides what upstream compiles in.
    LIBPQ_MAJOR = lib_version / 10000
    # LIBPQ_HAS_PIPELINING (libpq 14).
    PIPELINING = has("PQpipelineStatus")
    # LIBPQ_HAS_CHUNK_MODE (libpq 17), which also brings
    # `PQclosePrepared` and `PQcancelCreate`.
    CHUNK_MODE = has("PQsetChunkedRowsMode")
    # HAVE_PQRESULTMEMORYSIZE (libpq 12).
    RESULT_MEMORY_SIZE = has("PQresultMemorySize")
    # LIBPQ_HAS_FULL_PROTOCOL_VERSION (libpq 18).
    FULL_PROTOCOL_VERSION = has("PQfullProtocolVersion")

    INT = (-2**31)..(2**31 - 1)
    UINT = (-2**31)..(2**32 - 1)

    # pg.c's pg_enc_pg2ruby_mapping, in its order.
    ENCODINGS = [
      %w[UTF8 UTF-8], %w[BIG5 Big5], %w[EUC_CN GB2312], %w[EUC_JP EUC-JP],
      %w[EUC_JIS_2004 EUC-JP], %w[EUC_KR EUC-KR], %w[EUC_TW EUC-TW],
      %w[GB18030 GB18030], %w[GBK GBK], %w[ISO_8859_5 ISO-8859-5],
      %w[ISO_8859_6 ISO-8859-6], %w[ISO_8859_7 ISO-8859-7],
      %w[ISO_8859_8 ISO-8859-8], %w[KOI8 KOI8-R], %w[KOI8R KOI8-R],
      %w[KOI8U KOI8-U], %w[LATIN1 ISO-8859-1], %w[LATIN2 ISO-8859-2],
      %w[LATIN3 ISO-8859-3], %w[LATIN4 ISO-8859-4], %w[LATIN5 ISO-8859-9],
      %w[LATIN6 ISO-8859-10], %w[LATIN7 ISO-8859-13], %w[LATIN8 ISO-8859-14],
      %w[LATIN9 ISO-8859-15], %w[LATIN10 ISO-8859-16],
      %w[MULE_INTERNAL Emacs-Mule], %w[SJIS Windows-31J],
      %w[SHIFT_JIS_2004 Windows-31J], %w[UHC CP949], %w[WIN866 IBM866],
      %w[WIN874 Windows-874], %w[WIN1250 Windows-1250],
      %w[WIN1251 Windows-1251], %w[WIN1252 Windows-1252],
      %w[WIN1253 Windows-1253], %w[WIN1254 Windows-1254],
      %w[WIN1255 Windows-1255], %w[WIN1256 Windows-1256],
      %w[WIN1257 Windows-1257], %w[WIN1258 Windows-1258]
    ].map(&:freeze).freeze

    class << self
      # The entries the extension packed: Strings (BINARY) and nils.
      def unpack(packed)
        packed = packed.b
        out = []
        pos = 0
        size = packed.bytesize
        while pos < size
          len = packed.byteslice(pos, 4).unpack1("l<")
          pos += 4
          if len < 0
            out << nil
          else
            out << packed.byteslice(pos, len)
            pos += len
          end
        end
        out
      end

      # Strings and nils, packed for the extension.
      def pack(values)
        out = +"".b
        values.each do |value|
          if value.nil?
            out << [-1].pack("l<")
          else
            out << [value.bytesize].pack("l<") << value.b
          end
        end
        out
      end

      # PQconninfoOption's packed by the extension, as
      # pgconn_make_conninfo_array makes them.
      def conninfo_array(packed)
        fields = unpack(packed)
        keys = %i[keyword envvar compiled val label dispchar]
        fields.each_slice(7).map do |slice|
          hash = {}
          keys.each_with_index { |key, i| hash[key] = slice[i] if slice[i] }
          hash[:dispsize] = slice[6].to_i
          hash
        end
      end

      # The name `rb_obj_classname` and Check_Type give a value.
      def type_name(value)
        case value
        when nil then "nil"
        when true then "true"
        when false then "false"
        else value.class.to_s
        end
      end

      # `Check_Type(value, T_STRING)`.
      def check_string(value)
        return value if String === value
        raise TypeError, "wrong argument type #{type_name(value)} (expected String)"
      end

      # `Check_Type(value, T_ARRAY)`.
      def check_array(value)
        return value if Array === value
        raise TypeError, "wrong argument type #{type_name(value)} (expected Array)"
      end

      # `StringValue`.
      def string_value(value)
        return value if String === value
        if !value.nil? && value.respond_to?(:to_str)
          str = value.to_str
          return str if String === str
          raise TypeError, "can't convert #{value.class} to String (#{value.class}#to_str gives #{str.class})"
        end
        raise TypeError, "no implicit conversion of #{type_name(value)} into String"
      end

      # `StringValueCStr`: a String with no NUL in it.
      def c_string(value)
        value = string_value(value)
        raise ArgumentError, "string contains null byte" if value.include?("\0")
        value
      end

      # `rb_to_int`, the conversion NUM2INT and NUM2UINT make.
      def to_integer(value)
        case value
        when Integer then value
        when Float
          raise FloatDomainError, value.to_s if value.nan? || value.infinite?
          value.to_i
        when nil then raise TypeError, "no implicit conversion from nil to integer"
        when true, false then raise TypeError, "no implicit conversion of #{value} into Integer"
        else
          unless value.respond_to?(:to_int)
            raise TypeError, "no implicit conversion of #{value.class} into Integer"
          end
          int = value.to_int
          return int if Integer === int
          raise TypeError, "can't convert #{value.class} to Integer (#{value.class}#to_int gives #{int.class})"
        end
      end

      # `NUM2INT`.
      def num2int(value)
        int = to_integer(value)
        unless INT.cover?(int)
          raise RangeError, "integer #{int} too #{int < 0 ? "small" : "big"} to convert to 'int'"
        end
        int
      end

      # `NUM2UINT`, which takes a negative int and wraps it.
      def num2uint(value)
        int = to_integer(value)
        unless UINT.cover?(int)
          raise RangeError, "integer #{int} too #{int < 0 ? "small" : "big"} to convert to 'unsigned int'"
        end
        int & 0xffff_ffff
      end

      # `NUM2DBL`.
      def num2dbl(value)
        case value
        when Float then value
        when Integer then value.to_f
        when nil then raise TypeError, "can't convert nil into Float"
        when true, false then raise TypeError, "can't convert #{value} into Float"
        when String then raise TypeError, "no implicit conversion to float from string"
        else
          raise TypeError, "can't convert #{value.class} into Float" unless value.respond_to?(:to_f)
          value.to_f
        end
      end

      # `rb_obj_as_string`.
      def as_string(value)
        return value if String === value
        str = value.to_s
        String === str ? str : Kernel.instance_method(:to_s).bind_call(value)
      end

      # `rb_str_export_to_enc`: `str` in `enc`, or `str` itself when it is
      # in `enc` already or cannot be converted.
      def export(str, enc)
        return str if str.encoding == enc
        str.encode(enc)
      rescue EncodingError
        str
      end

      # pg_get_pg_encname_as_rb_encoding.
      def rb_encoding(pg_name)
        pair = ENCODINGS.find { |pg, _| pg == pg_name }
        return Encoding::BINARY unless pair
        Encoding.find(pair[1])
      rescue ArgumentError
        Encoding::BINARY
      end

      # pg_get_rb_encoding_as_pg_encoding: the last match, as the C's loop
      # keeps it, or SQL_ASCII.
      def pg_encoding(enc)
        name = enc.name
        found = nil
        ENCODINGS.each { |pg, rb| found = pg if rb == name }
        found || "SQL_ASCII"
      end

      # pg_raise_conn_error: `klass` with `message` (as `rb_sprintf` makes
      # it, BINARY) and `@connection` set.
      def raise_conn_error(klass, connection, message)
        error = klass.new(message.b)
        error.instance_variable_set(:@connection, connection)
        raise error
      end

      # `rb_check_frozen`.
      def check_frozen(object)
        return unless object.frozen?
        raise FrozenError.new("can't modify frozen #{object.class}: #{object.inspect}", receiver: object)
      end

      # pg_deprecated: `message` once per id, in verbose mode.
      def deprecated(id, message)
        @deprecated ||= ENV["PG_SKIP_DEPRECATION_WARNING"] ? :all : []
        return if @deprecated == :all || @deprecated.include?(id)
        @deprecated << id
        warn(message, uplevel: 2) if $VERBOSE
      end

      def conn_finalizer(handle)
        proc { conn_free(handle) }
      end

      def res_finalizer(handle)
        proc { res_free(handle) }
      end

      def cancon_finalizer(handle)
        proc { cancon_free(handle) }
      end
    end
  end

  # pg.c's constants.
  module Constants
    CONNECTION_OK = 0
    CONNECTION_BAD = 1
    CONNECTION_STARTED = 2
    CONNECTION_MADE = 3
    CONNECTION_AWAITING_RESPONSE = 4
    CONNECTION_AUTH_OK = 5
    CONNECTION_SETENV = 6
    CONNECTION_SSL_STARTUP = 7
    CONNECTION_NEEDED = 8
    CONNECTION_CHECK_WRITABLE = 9
    CONNECTION_CONSUME = 10
    CONNECTION_GSS_STARTUP = 11 if Spinel::LIBPQ_MAJOR >= 12
    CONNECTION_CHECK_TARGET = 12 if Spinel::LIBPQ_MAJOR >= 13
    CONNECTION_CHECK_STANDBY = 13 if Spinel::LIBPQ_MAJOR >= 14
    CONNECTION_ALLOCATED = 14 if Spinel::LIBPQ_MAJOR >= 17

    PGRES_POLLING_READING = 1
    PGRES_POLLING_WRITING = 2
    PGRES_POLLING_FAILED = 0
    PGRES_POLLING_OK = 3

    PQTRANS_IDLE = 0
    PQTRANS_ACTIVE = 1
    PQTRANS_INTRANS = 2
    PQTRANS_INERROR = 3
    PQTRANS_UNKNOWN = 4

    PQERRORS_TERSE = 0
    PQERRORS_DEFAULT = 1
    PQERRORS_VERBOSE = 2
    PQERRORS_SQLSTATE = 3 if Spinel::RESULT_MEMORY_SIZE

    PQSHOW_CONTEXT_NEVER = 0
    PQSHOW_CONTEXT_ERRORS = 1
    PQSHOW_CONTEXT_ALWAYS = 2

    PQPING_OK = 0
    PQPING_REJECT = 1
    PQPING_NO_RESPONSE = 2
    PQPING_NO_ATTEMPT = 3

    INV_WRITE = 0x00020000
    INV_READ = 0x00040000
    SEEK_SET = 0
    SEEK_CUR = 1
    SEEK_END = 2

    PGRES_EMPTY_QUERY = 0
    PGRES_COMMAND_OK = 1
    PGRES_TUPLES_OK = 2
    PGRES_COPY_OUT = 3
    PGRES_COPY_IN = 4
    PGRES_BAD_RESPONSE = 5
    PGRES_NONFATAL_ERROR = 6
    PGRES_FATAL_ERROR = 7
    PGRES_COPY_BOTH = 8
    PGRES_SINGLE_TUPLE = 9
    PGRES_TUPLES_CHUNK = 12 if Spinel::CHUNK_MODE
    if Spinel::PIPELINING
      PGRES_PIPELINE_SYNC = 10
      PGRES_PIPELINE_ABORTED = 11
    end

    PG_DIAG_SEVERITY = "S".ord
    PG_DIAG_SEVERITY_NONLOCALIZED = "V".ord
    PG_DIAG_SQLSTATE = "C".ord
    PG_DIAG_MESSAGE_PRIMARY = "M".ord
    PG_DIAG_MESSAGE_DETAIL = "D".ord
    PG_DIAG_MESSAGE_HINT = "H".ord
    PG_DIAG_STATEMENT_POSITION = "P".ord
    PG_DIAG_INTERNAL_POSITION = "p".ord
    PG_DIAG_INTERNAL_QUERY = "q".ord
    PG_DIAG_CONTEXT = "W".ord
    PG_DIAG_SOURCE_FILE = "F".ord
    PG_DIAG_SOURCE_LINE = "L".ord
    PG_DIAG_SOURCE_FUNCTION = "R".ord
    PG_DIAG_SCHEMA_NAME = "s".ord
    PG_DIAG_TABLE_NAME = "t".ord
    PG_DIAG_COLUMN_NAME = "c".ord
    PG_DIAG_DATATYPE_NAME = "d".ord
    PG_DIAG_CONSTRAINT_NAME = "n".ord

    if Spinel::PIPELINING
      PQ_PIPELINE_ON = 1
      PQ_PIPELINE_OFF = 0
      PQ_PIPELINE_ABORTED = 2
    end

    INVALID_OID = 0
    InvalidOid = 0
    DEF_PGPORT = 5432
  end

  # Not a binary gem: libpq is the system's, as for upstream's source gem.
  IS_BINARY_GEM = false

  include Constants

  class << self
    def library_version
      Spinel.lib_version
    end

    def isthreadsafe
      Spinel.isthreadsafe
    end
    alias_method :is_threadsafe?, :isthreadsafe
    alias_method :threadsafe?, :isthreadsafe

    def init_openssl(do_ssl, do_crypto)
      Spinel.init_openssl(Spinel.bool_int(do_ssl), Spinel.bool_int(do_crypto))
      nil
    end

    def init_ssl(do_ssl)
      Spinel.init_ssl(Spinel.bool_int(do_ssl))
      nil
    end
  end

  module Spinel
    # pg.c's pg_to_bool_int.
    def self.bool_int(value)
      case value
      when false then 0
      when true then 1
      else num2int(value)
      end
    end
  end
end

require "pg_ext/errors"
require "pg_ext/type_map"
require "pg_ext/coder"
require "pg_ext/connection"
require "pg_ext/result"
require "pg_ext/tuple"
require "pg_ext/cancel_connection"
