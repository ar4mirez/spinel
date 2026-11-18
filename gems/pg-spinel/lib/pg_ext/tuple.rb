# frozen_string_literal: true

# pg_tuple.c: PG::Tuple, one row of a result whose values are read from it
# when first asked for and kept, the result let go once each is read.

module PG
  class Tuple
    include Enumerable

    UNREAD = Object.new.freeze
    private_constant :UNREAD

    # pg_tuple_new.
    def self.__pg_new__(result, row, field_map, field_names)
      tuple = allocate
      tuple.__send__(:__pg_init__, result, row, field_map, field_names)
      tuple
    end
    private_class_method :__pg_new__

    private def __pg_init__(result, row, field_map, field_names)
      @__pg_result__ = result
      @__pg_row__ = row
      @__pg_field_map__ = field_map
      @__pg_values__ = Array.new(field_names.length, UNREAD)
      @__pg_field_names__ = field_map.size == field_names.length ? false : field_names
    end

    # pg_tuple_get_this.
    private def __pg_this__
      raise TypeError, "tuple is empty" unless @__pg_values__
      @__pg_values__
    end

    # pg_tuple_materialize_field.
    private def __pg_value__(col)
      value = @__pg_values__[col]
      if UNREAD.equal?(value)
        value = @__pg_result__.getvalue(@__pg_row__, col)
        @__pg_values__[col] = value
      end
      value
    end

    # pg_tuple_detach.
    private def __pg_detach__
      @__pg_result__ = nil
      @__pg_row__ = -1
    end

    private def __pg_materialize__
      @__pg_values__.length.times { |i| __pg_value__(i) }
      __pg_detach__
    end

    def fetch(key, *rest)
      values = __pg_this__
      raise ArgumentError, "wrong number of arguments (given #{rest.length + 1}, expected 1..2)" if rest.length > 1
      warn("block supersedes default value argument") if block_given? && rest.length == 1
      if Integer === key
        field_num = Spinel.num2int(key)
        field_num += values.length if field_num < 0
        if field_num < 0 || field_num >= values.length
          return yield(key) if block_given?
          raise IndexError, "Index #{field_num} is out of range" if rest.empty?
          return rest[0]
        end
      else
        index = @__pg_field_map__[key]
        if index.nil?
          return yield(key) if block_given?
          raise KeyError, "column not found" if rest.empty?
          return rest[0]
        end
        field_num = index
      end
      __pg_value__(field_num)
    end

    def [](key)
      values = __pg_this__
      if Integer === key
        field_num = Spinel.num2int(key)
        field_num += values.length if field_num < 0
        return nil if field_num < 0 || field_num >= values.length
      else
        field_num = @__pg_field_map__[key]
        return nil if field_num.nil?
      end
      __pg_value__(field_num)
    end

    def each
      values = __pg_this__
      return enum_for(:each) { values.length } unless block_given?
      if @__pg_field_names__
        @__pg_field_names__.each_with_index { |name, i| yield name, __pg_value__(i) }
      else
        @__pg_field_map__.each { |name, i| yield name, __pg_value__(i) }
      end
      __pg_detach__
      self
    end

    def each_value
      values = __pg_this__
      return enum_for(:each_value) { values.length } unless block_given?
      values.length.times { |i| yield __pg_value__(i) }
      __pg_detach__
      self
    end

    def values
      __pg_this__
      __pg_materialize__
      @__pg_values__.dup
    end

    def length
      __pg_this__.length
    end
    alias_method :size, :length

    def index(key)
      __pg_this__
      @__pg_field_map__[key]
    end

    private def field_map
      __pg_this__
      @__pg_field_map__
    end

    private def field_names
      __pg_this__
      @__pg_field_names__
    end

    private def marshal_dump
      __pg_this__
      __pg_materialize__
      names = @__pg_field_names__ || @__pg_field_map__.keys
      [names, @__pg_values__.dup]
    end

    private def marshal_load(array)
      Spinel.check_frozen(self)
      raise TypeError, "tuple is not empty" if @__pg_values__
      Spinel.check_array(array)
      raise TypeError, "expected an array of 2 elements" unless array.length == 2
      names = Spinel.check_array(array[0]).freeze
      values = Spinel.check_array(array[1])
      raise TypeError, "different number of fields and values" unless names.length == values.length
      map = {}
      names.each_with_index { |name, i| map[name] = i }
      @__pg_result__ = nil
      @__pg_row__ = -1
      @__pg_field_map__ = map.freeze
      @__pg_values__ = values.dup
      @__pg_field_names__ = map.size == names.length ? false : names
      self
    end
  end
end
