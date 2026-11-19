# frozen_string_literal: true

# pg_coder.c, pg_copy_coder.c and pg_record_coder.c's classes, with their
# attributes, which upstream's `lib/pg/coder.rb` builds on; and the coder
# classes of pg_text_encoder.c, pg_text_decoder.c, pg_binary_encoder.c and
# pg_binary_decoder.c, by name.
#
# ponytail: a coder's `encode` and `decode` raise NotImplementedError, as
# nothing here turns a value into its PostgreSQL text or binary form yet.
# Upgrade: each coder's function in Ruby, after the C's.

module PG
  class Coder
    TIMESTAMP_DB_UTC = 0x0
    TIMESTAMP_DB_LOCAL = 0x1
    TIMESTAMP_APP_UTC = 0x0
    TIMESTAMP_APP_LOCAL = 0x2
    FORMAT_ERROR_MASK = 0xc
    FORMAT_ERROR_TO_RAISE = 0x4
    FORMAT_ERROR_TO_STRING = 0x8
    FORMAT_ERROR_TO_PARTIAL = 0xc

    module BinaryFormatting; end

    attr_accessor :name

    def self.allocate
      raise TypeError, "PG::Coder cannot be instantiated directly" if self == Coder || !(self <= Spinel::Allocatable)
      super
    end

    def oid=(oid)
      Spinel.check_frozen(self)
      @__pg_oid__ = Spinel.num2uint(oid)
      oid
    end

    def oid
      @__pg_oid__ || 0
    end

    def format=(format)
      Spinel.check_frozen(self)
      @__pg_format__ = Spinel.num2int(format)
      format
    end

    def format
      @__pg_format__ || 0
    end

    def flags=(flags)
      Spinel.check_frozen(self)
      @__pg_flags__ = Spinel.num2int(flags)
      flags
    end

    def flags
      @__pg_flags__ || 0
    end
  end

  module Spinel
    # What a coder class whose allocator upstream defines includes, and
    # what each of upstream's named coders is.
    module Allocatable
      def initialize(...)
        @name = nil unless defined?(@name)
        super
      end
    end

    # A named coder: `encode` or `decode`, not here yet.
    module Unimplemented
      def encode(value, encoding = nil)
        return nil if value.nil?
        raise NotImplementedError, "#{self.class} is not in pg-spinel yet; values come back as Strings"
      end

      def decode(value, tuple = nil, field = nil)
        return nil if value.nil?
        raise NotImplementedError, "#{self.class} is not in pg-spinel yet; values come back as Strings"
      end
    end

    module NoEncoder
      def encode(value, encoding = nil)
        return nil if value.nil?
        raise RuntimeError, "no encoder function defined"
      end
    end

    module NoDecoder
      def decode(value, tuple = nil, field = nil)
        return nil if value.nil?
        raise RuntimeError, "no decoder function defined"
      end
    end
  end

  class SimpleCoder < Coder; end

  class SimpleEncoder < SimpleCoder
    include Spinel::Allocatable
    include Spinel::NoEncoder
  end

  class SimpleDecoder < SimpleCoder
    include Spinel::Allocatable
    include Spinel::NoDecoder
  end

  class CompositeCoder < Coder
    attr_reader :elements_type

    def elements_type=(elements_type)
      Spinel.check_frozen(self)
      unless elements_type.nil? || Coder === elements_type
        raise TypeError, "wrong argument type #{Spinel.type_name(elements_type)} (expected PG::Coder)"
      end
      @elements_type = elements_type
    end

    def needs_quotation=(needs_quotation)
      Spinel.check_frozen(self)
      @__pg_needs_quotation__ = needs_quotation ? true : false
      needs_quotation
    end

    def needs_quotation?
      defined?(@__pg_needs_quotation__) ? @__pg_needs_quotation__ : true
    end

    def delimiter=(delimiter)
      Spinel.check_frozen(self)
      delimiter = Spinel.string_value(delimiter)
      raise ArgumentError, "delimiter size must be one byte" unless delimiter.bytesize == 1
      @__pg_delimiter__ = delimiter.b
      delimiter
    end

    def delimiter
      (@__pg_delimiter__ || ",").b
    end

    def dimensions=(dimensions)
      Spinel.check_frozen(self)
      if !dimensions.nil? && Spinel.num2int(dimensions) < 0
        raise ArgumentError, "dimensions must be nil or >= 0"
      end
      @__pg_dimensions__ = dimensions.nil? ? nil : Spinel.num2int(dimensions)
      dimensions
    end

    def dimensions
      @__pg_dimensions__
    end
  end

  class CompositeEncoder < CompositeCoder
    include Spinel::Allocatable
    include Spinel::NoEncoder

    def initialize(...)
      @elements_type = nil
      super
    end
  end

  class CompositeDecoder < CompositeCoder
    include Spinel::Allocatable
    include Spinel::NoDecoder

    def initialize(...)
      @elements_type = nil
      super
    end
  end

  class CopyCoder < Coder
    def type_map=(type_map)
      Spinel.check_frozen(self)
      Spinel.check_type_map(type_map)
      @__pg_type_map__ = type_map
    end

    def type_map
      defined?(@__pg_type_map__) ? @__pg_type_map__ : Spinel::ALL_STRINGS
    end

    def delimiter=(delimiter)
      Spinel.check_frozen(self)
      delimiter = Spinel.string_value(delimiter)
      raise ArgumentError, "delimiter size must be one byte" unless delimiter.bytesize == 1
      @__pg_delimiter__ = delimiter.b
      delimiter
    end

    def delimiter
      (@__pg_delimiter__ || "\t").b
    end

    def null_string=(null_string)
      Spinel.check_frozen(self)
      @__pg_null_string__ = Spinel.string_value(null_string)
    end

    def null_string
      @__pg_null_string__ ||= "\\N".b
    end
  end

  class CopyEncoder < CopyCoder
    include Spinel::Allocatable
  end

  class CopyDecoder < CopyCoder
    include Spinel::Allocatable
  end

  class RecordCoder < Coder
    def type_map=(type_map)
      Spinel.check_frozen(self)
      Spinel.check_type_map(type_map)
      @__pg_type_map__ = type_map
    end

    def type_map
      defined?(@__pg_type_map__) ? @__pg_type_map__ : Spinel::ALL_STRINGS
    end
  end

  class RecordEncoder < RecordCoder
    include Spinel::Allocatable
  end

  class RecordDecoder < RecordCoder
    include Spinel::Allocatable
  end

  module TextEncoder; end
  module TextDecoder; end
  module BinaryEncoder; end
  module BinaryDecoder; end

  # Each of upstream's coders: its module, name and base, in pg_define_coder
  # order.
  [
    [TextEncoder, %i[Boolean Integer Float String Bytea Identifier Numeric], SimpleEncoder],
    [TextEncoder, %i[Array QuotedLiteral ToBase64], CompositeEncoder],
    [TextEncoder, %i[CopyRow], CopyEncoder],
    [TextEncoder, %i[Record], RecordEncoder],
    [TextDecoder, %i[Boolean Integer Float String Bytea Identifier Timestamp Numeric Inet], SimpleDecoder],
    [TextDecoder, %i[Array FromBase64], CompositeDecoder],
    [TextDecoder, %i[CopyRow], CopyDecoder],
    [TextDecoder, %i[Record], RecordDecoder],
    [BinaryEncoder, %i[Boolean Int2 Int4 Int8 Float4 Float8 String Bytea Timestamp Date], SimpleEncoder],
    [BinaryEncoder, %i[Array FromBase64], CompositeEncoder],
    [BinaryEncoder, %i[CopyRow], CopyEncoder],
    [BinaryDecoder, %i[Boolean Integer Float String Bytea Timestamp Date], SimpleDecoder],
    [BinaryDecoder, %i[Array ToBase64], CompositeDecoder],
    [BinaryDecoder, %i[CopyRow], CopyDecoder]
  ].each do |namespace, names, base|
    names.each do |name|
      klass = namespace.const_set(name, Class.new(base))
      klass.include(Spinel::Unimplemented)
      klass.include(Coder::BinaryFormatting) if namespace == BinaryEncoder || namespace == BinaryDecoder
    end
  end
end
