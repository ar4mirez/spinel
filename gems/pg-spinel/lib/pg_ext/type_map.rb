# frozen_string_literal: true

# pg_type_map.c and pg_type_map_all_strings.c: PG::TypeMap, which maps
# nothing, and PG::TypeMapAllStrings, every connection's and result's
# default, which answers a text value as a String in the connection's
# encoding, a binary one as a BINARY String, and sends a parameter as its
# `to_s`.
#
# ponytail: TypeMapByClass, TypeMapByColumn, TypeMapByMriType, TypeMapByOid
# and TypeMapInRuby exist and refuse to be made (NotImplementedError).
# Upgrade: each in Ruby over Result's Strings and the coders (pg_ext.rb).

module PG
  class TypeMap
    module DefaultTypeMappable
      def default_type_map=(typemap)
        Spinel.check_frozen(self)
        Spinel.check_type_map(typemap)
        @__pg_default_type_map__ = typemap
      end

      def default_type_map
        @__pg_default_type_map__
      end

      def with_default_type_map(typemap)
        self.default_type_map = typemap
        self
      end
    end

    def query_param_encoders(params)
      Spinel.check_array(params)
      __pg_fit_to_query__(params)
      params.map { nil }
    end

    # pg_typemap_fit_to_result.
    def __pg_fit_to_result__(_result)
      raise NotImplementedError, "type map #{self.class} is not suitable to map result values"
    end

    # pg_typemap_fit_to_query.
    def __pg_fit_to_query__(_params)
      raise NotImplementedError, "type map #{self.class} is not suitable to map query params"
    end
  end

  class TypeMapAllStrings < TypeMap
    def __pg_fit_to_result__(_result)
      self
    end

    def __pg_fit_to_query__(_params)
      self
    end
  end

  %i[TypeMapByClass TypeMapByColumn TypeMapByMriType TypeMapByOid TypeMapInRuby].each do |name|
    klass = const_set(name, Class.new(TypeMap))
    klass.include(TypeMap::DefaultTypeMappable) unless name == :TypeMapByMriType
    klass.define_singleton_method(:allocate) do
      raise NotImplementedError, "PG::#{name} is not in pg-spinel yet; values come back as Strings"
    end
    klass.define_method(:initialize) do |*|
      raise NotImplementedError, "PG::#{name} is not in pg-spinel yet; values come back as Strings"
    end
  end

  module Spinel
    # The one PG::TypeMapAllStrings every connection starts with, frozen.
    ALL_STRINGS = TypeMapAllStrings.new.freeze

    # TypedData_Get_Struct(..., &pg_typemap_type, ...).
    def self.check_type_map(value)
      return value if TypeMap === value
      raise TypeError, "wrong argument type #{type_name(value)} (expected PG::TypeMap)"
    end
  end
end
