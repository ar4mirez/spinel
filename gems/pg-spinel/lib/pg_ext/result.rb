# frozen_string_literal: true

# pg_result.c: PG::Result's C methods, which upstream's `lib/pg/result.rb`
# builds `inspect` and the rest on. Its libpq result is a box held by handle
# in `@__pg_res__`; the connection, encoding, type map and field names are
# the struct's other fields. Under PG::TypeMapAllStrings a value is a String
# in the connection's encoding, a binary one BINARY, and SQL NULL nil.

module PG
  class Result
    include Enumerable
    include Constants

    class << self
      def allocate
        raise TypeError, "allocator undefined for PG::Result"
      end

      def new(*)
        raise TypeError, "allocator undefined for PG::Result"
      end

      def res_status(status)
        Spinel.res_status_name(Spinel.num2int(status)).force_encoding(Encoding::UTF_8)
      end

      # pg_new_result: a Result over `handle` (or none, for nil), with what
      # it takes from `connection`.
      private def __pg_new__(handle, connection)
        result = Class.instance_method(:allocate).bind_call(self)
        result.__send__(:__pg_init__, handle, connection)
        result
      end
    end

    private def __pg_init__(handle, connection)
      @__pg_res__ = handle
      @__pg_connection__ = connection
      @__pg_autoclear__ = false
      @__pg_type_map__ = Spinel::ALL_STRINGS
      if handle
        ObjectSpace.define_finalizer(self, Spinel.res_finalizer(handle))
        @__pg_enc__ = connection.__send__(:__pg_enc__)
        @__pg_symbols__ = connection.field_name_type == :symbol
        @__pg_type_map__ = connection.type_map_for_results.__pg_fit_to_result__(self)
      else
        @__pg_enc__ = Encoding.find("locale")
        @__pg_symbols__ = false
      end
    end

    # pgresult_get: the handle of a result not cleared.
    private def __pg_get__
      h = @__pg_res__
      raise PG::Error, "result has been cleared" if h.nil? || Spinel.res_cleared?(h)
      h
    end

    private def __pg_live__?
      h = @__pg_res__
      !h.nil? && !Spinel.res_cleared?(h)
    end

    # pg_cstr_to_sym: a field name as a frozen, interned String in the
    # result's encoding, or a Symbol.
    private def __pg_field_name__(bytes)
      name = bytes.force_encoding(@__pg_enc__)
      @__pg_symbols__ ? name.to_sym : -name
    end

    # pgresult_init_fnames.
    private def __pg_fnames__
      h = __pg_get__
      @__pg_fnames__ ||= Spinel.unpack(Spinel.res_fnames(h)).map { |n| __pg_field_name__(n) }.freeze
    end

    # The column formats, for typecasting a value (pg_tmas_typecast_result_value).
    private def __pg_formats__
      @__pg_formats__ ||= Spinel.res_formats(__pg_get__).bytes.freeze
    end

    private def __pg_cast__(bytes, column)
      return nil if bytes.nil?
      __pg_formats__[column] == 0 ? bytes.force_encoding(@__pg_enc__) : bytes
    end

    # The values of row `row`, cast.
    private def __pg_row__(row)
      Spinel.unpack(Spinel.res_row(__pg_get__, row)).each_with_index.map { |v, i| __pg_cast__(v, i) }
    end

    # ensure_init_for_tuple: the frozen Hash of field name to column.
    private def __pg_field_map__
      @__pg_field_map__ ||= begin
        map = {}
        __pg_fnames__.each_with_index { |name, i| map[name] = i }
        map.freeze
      end
    end

    def result_status
      Spinel.res_status(__pg_get__)
    end

    def res_status(*args)
      __pg_get__
      raise ArgumentError, "only 0 or 1 arguments expected" if args.length > 1
      status = args.empty? ? Spinel.res_status(@__pg_res__) : Spinel.num2int(args[0])
      Spinel.res_status_name(status).force_encoding(@__pg_enc__)
    end

    def error_message
      Spinel.res_error_message(__pg_get__).force_encoding(@__pg_enc__)
    end
    alias_method :result_error_message, :error_message

    def verbose_error_message(verbosity, show_context)
      h = __pg_get__
      message = Spinel.res_verbose_error_message(h, Spinel.num2int(verbosity), Spinel.num2int(show_context))
      raise NoMemoryError, "insufficient memory to format error message" if message.nil?
      message.force_encoding(@__pg_enc__)
    end
    alias_method :result_verbose_error_message, :verbose_error_message

    def error_field(field)
      h = __pg_get__
      Spinel.res_error_field(h, Spinel.num2int(field))&.force_encoding(@__pg_enc__)
    end
    alias_method :result_error_field, :error_field

    # pg_result_check.
    def check
      if __pg_live__?
        case Spinel.res_status(@__pg_res__)
        when PGRES_TUPLES_OK, PGRES_COPY_OUT, PGRES_COPY_IN, PGRES_COPY_BOTH, PGRES_SINGLE_TUPLE,
             PGRES_EMPTY_QUERY, PGRES_COMMAND_OK
          return self
        when Spinel::PIPELINING && PGRES_PIPELINE_SYNC, Spinel::CHUNK_MODE && PGRES_TUPLES_CHUNK
          return self
        when PGRES_BAD_RESPONSE, PGRES_FATAL_ERROR, PGRES_NONFATAL_ERROR,
             Spinel::PIPELINING && PGRES_PIPELINE_ABORTED
          error = Spinel.res_error_message(@__pg_res__)
        else
          error = +"internal error : unknown result status."
        end
        sqlstate = Spinel.res_error_field(@__pg_res__, PG_DIAG_SQLSTATE)
      else
        error = Spinel.conn_error_message(@__pg_connection__.__send__(:__pg_conn__))
        sqlstate = nil
      end
      error.force_encoding(@__pg_enc__)
      exception = Spinel.error_class(sqlstate).new(error)
      exception.instance_variable_set(:@connection, @__pg_connection__)
      exception.instance_variable_set(:@result, __pg_live__? ? self : nil)
      raise exception
    end

    def clear
      Spinel.check_frozen(self)
      h = @__pg_res__
      Spinel.res_clear(h) if h && !@__pg_autoclear__
      @__pg_fnames__ = nil
      @__pg_formats__ = nil
      @__pg_res__ = nil
      nil
    end

    def freeze
      __pg_field_map__
      __pg_formats__
      @__pg_connection__ = nil
      super
    end

    def cleared?
      !__pg_live__?
    end

    def autoclear?
      @__pg_autoclear__
    end

    def ntuples
      Spinel.res_ntuples(__pg_get__)
    end
    alias_method :num_tuples, :ntuples

    def nfields
      Spinel.res_nfields(__pg_get__)
    end
    alias_method :num_fields, :nfields

    def binary_tuples
      Spinel.res_binary_tuples(__pg_get__)
    end

    def fname(index)
      h = __pg_get__
      i = Spinel.num2int(index)
      raise ArgumentError, "invalid field number #{i}" if i < 0 || i >= Spinel.res_nfields(h)
      __pg_field_name__(Spinel.res_fname(h, i))
    end

    def fnumber(name)
      Spinel.check_string(name)
      h = __pg_get__
      n = Spinel.res_fnumber(h, Spinel.c_string(name))
      raise ArgumentError, "Unknown field: #{name}" if n == -1
      n
    end

    def ftable(column_number)
      col = Spinel.num2int(column_number)
      h = __pg_get__
      raise ArgumentError, "Invalid column index: #{col}" if col < 0 || col >= Spinel.res_nfields(h)
      Spinel.res_ftable(h, col)
    end

    def ftablecol(column_number)
      col = Spinel.num2int(column_number)
      h = __pg_get__
      raise ArgumentError, "Invalid column index: #{col}" if col < 0 || col >= Spinel.res_nfields(h)
      Spinel.res_ftablecol(h, col)
    end

    def fformat(column_number)
      h = __pg_get__
      col = Spinel.num2int(column_number)
      raise ArgumentError, "Column number is out of range: #{col}" if col < 0 || col >= Spinel.res_nfields(h)
      Spinel.res_fformat(h, col)
    end

    def ftype(index)
      h = __pg_get__
      i = Spinel.num2int(index)
      raise ArgumentError, "invalid field number #{i}" if i < 0 || i >= Spinel.res_nfields(h)
      Spinel.res_ftype(h, i)
    end

    def fmod(column_number)
      h = __pg_get__
      col = Spinel.num2int(column_number)
      raise ArgumentError, "Column number is out of range: #{col}" if col < 0 || col >= Spinel.res_nfields(h)
      Spinel.res_fmod(h, col)
    end

    def fsize(index)
      i = Spinel.num2int(index)
      h = __pg_get__
      raise ArgumentError, "invalid field number #{i}" if i < 0 || i >= Spinel.res_nfields(h)
      Spinel.res_fsize(h, i)
    end

    def getvalue(tup_num, field_num)
      h = __pg_get__
      i = Spinel.num2int(tup_num)
      j = Spinel.num2int(field_num)
      raise ArgumentError, "invalid tuple number #{i}" if i < 0 || i >= Spinel.res_ntuples(h)
      raise ArgumentError, "invalid field number #{j}" if j < 0 || j >= Spinel.res_nfields(h)
      __pg_cast__(Spinel.res_getvalue(h, i, j), j)
    end

    def getisnull(tup_num, field_num)
      i = Spinel.num2int(tup_num)
      j = Spinel.num2int(field_num)
      h = __pg_get__
      raise ArgumentError, "invalid tuple number #{i}" if i < 0 || i >= Spinel.res_ntuples(h)
      raise ArgumentError, "invalid field number #{j}" if j < 0 || j >= Spinel.res_nfields(h)
      Spinel.res_getisnull(h, i, j)
    end

    def getlength(tup_num, field_num)
      i = Spinel.num2int(tup_num)
      j = Spinel.num2int(field_num)
      h = __pg_get__
      raise ArgumentError, "invalid tuple number #{i}" if i < 0 || i >= Spinel.res_ntuples(h)
      raise ArgumentError, "invalid field number #{j}" if j < 0 || j >= Spinel.res_nfields(h)
      Spinel.res_getlength(h, i, j)
    end

    def nparams
      Spinel.res_nparams(__pg_get__)
    end

    def paramtype(param_number)
      h = __pg_get__
      Spinel.res_paramtype(h, Spinel.num2int(param_number))
    end

    def cmd_status
      Spinel.res_cmd_status(__pg_get__).force_encoding(@__pg_enc__)
    end

    # strtol on PQcmdTuples: its leading digits, 0 for none.
    def cmd_tuples
      Spinel.res_cmd_tuples(__pg_get__).to_i
    end
    alias_method :cmdtuples, :cmd_tuples

    def oid_value
      n = Spinel.res_oid_value(__pg_get__)
      n == 0 ? nil : n
    end

    def [](index)
      h = __pg_get__
      tuple_num = Spinel.num2int(index)
      names = __pg_fnames__
      raise IndexError, "Index #{tuple_num} is out of range" if tuple_num < 0 || tuple_num >= Spinel.res_ntuples(h)
      tuple = {}
      __pg_row__(tuple_num).each_with_index { |value, i| tuple[names[i]] = value }
      tuple
    end

    def each_row
      return enum_for(:each_row) { ntuples } unless block_given?
      h = __pg_get__
      Spinel.res_ntuples(h).times { |row| yield __pg_row__(row) }
      nil
    end

    def values
      h = __pg_get__
      nfields = Spinel.res_nfields(h)
      return Array.new(Spinel.res_ntuples(h)) { [] } if nfields == 0
      Spinel.unpack(Spinel.res_rows(h)).each_slice(nfields).map do |row|
        row.each_with_index.map { |v, i| __pg_cast__(v, i) }
      end
    end

    # make_column_result_array.
    private def __pg_column__(col)
      h = __pg_get__
      raise IndexError, "no column #{col} in result" if col >= Spinel.res_nfields(h)
      Spinel.unpack(Spinel.res_column(h, col)).map { |v| __pg_cast__(v, col) }
    end

    def column_values(index)
      __pg_column__(Spinel.num2int(index))
    end

    def field_values(field)
      h = __pg_get__
      field = field.to_s if Symbol === field
      fieldname = Spinel.c_string(field)
      fnum = Spinel.res_fnumber(h, fieldname)
      raise IndexError, "no such field '#{fieldname}' in result" if fnum < 0
      __pg_column__(fnum)
    end

    def tuple_values(index)
      tuple_num = Spinel.num2int(index)
      h = __pg_get__
      raise IndexError, "Index #{tuple_num} is out of range" if tuple_num < 0 || tuple_num >= Spinel.res_ntuples(h)
      __pg_row__(tuple_num)
    end

    def tuple(index)
      tuple_num = Spinel.num2int(index)
      h = __pg_get__
      raise IndexError, "Index #{tuple_num} is out of range" if tuple_num < 0 || tuple_num >= Spinel.res_ntuples(h)
      Tuple.__send__(:__pg_new__, self, tuple_num, __pg_field_map__, __pg_fnames__)
    end

    def each
      return enum_for(:each) { ntuples } unless block_given?
      h = __pg_get__
      tuple_num = 0
      while tuple_num < Spinel.res_ntuples(h)
        yield self[tuple_num]
        tuple_num += 1
      end
      self
    end

    def each_tuple
      return enum_for(:each_tuple) { ntuples } unless block_given?
      h = __pg_get__
      tuple_num = 0
      while tuple_num < Spinel.res_ntuples(h)
        yield tuple(tuple_num)
        tuple_num += 1
      end
      self
    end

    def fields
      __pg_fnames__.dup
    end

    def type_map=(typemap)
      Spinel.check_frozen(self)
      Spinel.check_type_map(typemap)
      @__pg_type_map__ = typemap.__pg_fit_to_result__(self)
    end

    def type_map
      @__pg_type_map__
    end

    # pgresult_stream_any: yield each row of a single-row (or chunked)
    # result, then the next result's, until the query's end.
    private def __pg_stream__(how)
      Spinel.check_frozen(self)
      return enum_for(how) unless block_given?
      h = __pg_get__
      conn = @__pg_connection__
      pgconn = conn.__send__(:__pg_conn__)
      nfields = Spinel.res_nfields(h)
      loop do
        ntuples = Spinel.res_ntuples(h)
        case Spinel.res_status(h)
        when PGRES_TUPLES_OK, PGRES_COMMAND_OK
          return self if ntuples == 0
          raise InvalidResultStatus, "PG::Result is not in single row mode"
        when PGRES_SINGLE_TUPLE, Spinel::CHUNK_MODE && PGRES_TUPLES_CHUNK
          nil
        else
          check
        end
        nfields2 = Spinel.res_nfields(h)
        if nfields != nfields2
          clear
          raise InvalidChangeOfResultFields, "number of fields changed in single row mode from #{nfields} to #{nfields2} - this is a sign for intersection with another query"
        end
        # The block answers whether it is done with the result, which is
        # then freed; when not, a copy holding it took it over.
        Spinel.res_free(h) if yield(ntuples)
        ObjectSpace.undefine_finalizer(self)
        @__pg_res__ = nil
        @__pg_fnames__ = nil
        @__pg_formats__ = nil
        conn.block if Spinel.conn_is_busy(pgconn)
        h = Spinel.conn_get_result(pgconn)
        raise NoResultError, "no result received - possibly an intersection with another query" if h.nil?
        @__pg_res__ = h
        ObjectSpace.define_finalizer(self, Spinel.res_finalizer(h))
      end
    end

    def stream_each
      return enum_for(:stream_each) unless block_given?
      __pg_stream__(:stream_each) do |ntuples|
        ntuples.times { |n| yield self[n] }
        true
      end
    end

    def stream_each_row
      return enum_for(:stream_each_row) unless block_given?
      __pg_stream__(:stream_each_row) do |ntuples|
        ntuples.times { |n| yield __pg_row__(n) }
        true
      end
    end

    def stream_each_tuple
      return enum_for(:stream_each_tuple) unless block_given?
      __pg_field_map__
      __pg_stream__(:stream_each_tuple) do |ntuples|
        copy = Result.__send__(:__pg_new__, @__pg_res__, @__pg_connection__)
        copy.instance_variable_set(:@__pg_symbols__, @__pg_symbols__)
        ntuples.times { |n| yield copy.tuple(n) }
        false
      end
    end

    def field_name_type=(sym)
      Spinel.check_frozen(self)
      raise ArgumentError, "field names are already materialized" if @__pg_fnames__
      unless sym == :symbol || sym == :string
        raise ArgumentError, "invalid argument #{sym.inspect}"
      end
      @__pg_symbols__ = sym == :symbol
      sym
    end

    def field_name_type
      @__pg_symbols__ ? :symbol : :string
    end
  end
end
