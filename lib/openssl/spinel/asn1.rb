# frozen_string_literal: true

# `ossl_asn1.c` on Spinel (#107): `OpenSSL::ASN1`, its data classes, and DER
# both ways. All of it is Ruby: DER is bytes, and the one thing asked of
# OpenSSL is what an object identifier is called (`__openssl_tls__("obj")`,
# `OBJ_txt2obj`), so a name resolves as it does on CRuby.
#
# ponytail: `ASN1.traverse`, `ObjectId.register` and BER's indefinite
# length on output are not here. Upgrade: `traverse` over the decoder's
# offsets, and `OBJ_create` behind a primitive for `register`.

module OpenSSL
  module ASN1
    class ASN1Error < OpenSSLError; end

    UNIVERSAL_TAG_NAME = [
      "EOC", "BOOLEAN", "INTEGER", "BIT_STRING", "OCTET_STRING", "NULL", "OBJECT",
      "OBJECT_DESCRIPTOR", "EXTERNAL", "REAL", "ENUMERATED", "EMBEDDED_PDV", "UTF8STRING",
      "RELATIVE_OID", nil, nil, "SEQUENCE", "SET", "NUMERICSTRING", "PRINTABLESTRING",
      "T61STRING", "VIDEOTEXSTRING", "IA5STRING", "UTCTIME", "GENERALIZEDTIME",
      "GRAPHICSTRING", "ISO64STRING", "GENERALSTRING", "UNIVERSALSTRING", "CHARACTER_STRING",
      "BMPSTRING",
    ].freeze
    UNIVERSAL_TAG_NAME.each_with_index { |name, tag| const_set(name, tag) unless name.nil? }

    # The tag class, in a tag's first octet.
    CLASS_BITS = { UNIVERSAL: 0x00, APPLICATION: 0x40, CONTEXT_SPECIFIC: 0x80, PRIVATE: 0xc0 }.freeze
    private_constant :CLASS_BITS

    # `[nid, sn, ln, dotted]` for a name or OID, or nil; with `message`, an
    # ASN1Error of it instead of nil.
    def self.__obj__(text, message = nil)
      info = __openssl_tls__("obj", text, message)
      raise OpenSSL.__send__(:__error__, info[1]) if Array === info && info[0] == :__failure__
      info
    end
    private_class_method :__obj__

    class ASN1Data
      attr_accessor :value, :tag, :tag_class, :indefinite_length
      alias infinite_length indefinite_length
      alias infinite_length= indefinite_length=

      def initialize(value, tag, tag_class)
        raise ASN1Error, "invalid tag class" unless Symbol === tag_class
        @tag = tag
        @value = value
        @tag_class = tag_class
        @indefinite_length = false
      end

      def to_der
        if Array === @value
          content = @value.map { |v| v.to_der }.join.b
          ASN1.__send__(:__header__, @tag_class, @tag, true, content.bytesize) + content
        else
          content = String.new(@value.to_s, encoding: Encoding::BINARY)
          ASN1.__send__(:__header__, @tag_class, @tag, false, content.bytesize) + content
        end
      end
    end

    class Primitive < ASN1Data
      attr_accessor :tagging

      def initialize(*args)
        unless (1..4).cover?(args.size)
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..4)"
        end
        value, tag, tagging, tag_class = args
        default = self.class.__default_tag__
        if default.nil? || args.size > 1
          raise ASN1Error, "must specify tag number" if tag.nil?
          raise ASN1Error, "invalid tagging method" unless tagging.nil? || Symbol === tagging
          tag_class = tagging.nil? ? :UNIVERSAL : :CONTEXT_SPECIFIC if tag_class.nil?
          raise ASN1Error, "invalid tag class" unless Symbol === tag_class
        else
          tag = default
          tagging = nil
          tag_class = :UNIVERSAL
        end
        @tag = tag
        @value = value
        @tagging = tagging
        @tag_class = tag_class
        @indefinite_length = false
        @unused_bits = 0 if default == BIT_STRING
      end

      def self.__default_tag__ = nil

      def to_der
        content = __content__
        ASN1.__send__(:__wrap__, self, content, false)
      end

      private

      def __content__ = String.new(@value.to_s, encoding: Encoding::BINARY)
    end

    class Constructive < ASN1Data
      include Enumerable
      attr_accessor :tagging

      def initialize(*args)
        unless (1..4).cover?(args.size)
          raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..4)"
        end
        value, tag, tagging, tag_class = args
        default = self.class.__default_tag__
        if default.nil? || args.size > 1
          raise ASN1Error, "must specify tag number" if tag.nil?
          raise ASN1Error, "invalid tagging method" unless tagging.nil? || Symbol === tagging
          tag_class = tagging.nil? ? :UNIVERSAL : :CONTEXT_SPECIFIC if tag_class.nil?
          raise ASN1Error, "invalid tag class" unless Symbol === tag_class
        else
          tag = default
          tagging = nil
          tag_class = :UNIVERSAL
        end
        @tag = tag
        @value = value
        @tagging = tagging
        @tag_class = tag_class
        @indefinite_length = false
      end

      def self.__default_tag__ = nil

      def each(&block)
        return enum_for(:each) unless block
        @value.each(&block)
        self
      end

      def to_der
        items = @value.reject { |v| ASN1::EndOfContent === v }
        content = items.map { |v| v.to_der }.join.b
        ASN1.__send__(:__wrap__, self, content, true)
      end
    end

    # The universal classes, each with the tag it defaults to.
    {
      EndOfContent: EOC, Boolean: BOOLEAN, Integer: INTEGER, Enumerated: ENUMERATED,
      BitString: BIT_STRING, OctetString: OCTET_STRING, UTF8String: UTF8STRING,
      NumericString: NUMERICSTRING, PrintableString: PRINTABLESTRING, T61String: T61STRING,
      VideotexString: VIDEOTEXSTRING, IA5String: IA5STRING, GraphicString: GRAPHICSTRING,
      ISO64String: ISO64STRING, GeneralString: GENERALSTRING, UniversalString: UNIVERSALSTRING,
      BMPString: BMPSTRING, Null: NULL, ObjectId: OBJECT, UTCTime: UTCTIME,
      GeneralizedTime: GENERALIZEDTIME,
    }.each do |name, tag|
      klass = Class.new(Primitive)
      klass.define_singleton_method(:__default_tag__) { tag }
      const_set(name, klass)
    end
    { Sequence: SEQUENCE, Set: SET }.each do |name, tag|
      klass = Class.new(Constructive)
      klass.define_singleton_method(:__default_tag__) { tag }
      const_set(name, klass)
    end

    class EndOfContent
      def initialize
        super("", 0, nil, :UNIVERSAL)
      end

      def to_der = "\x00\x00".b
    end

    class BitString
      attr_accessor :unused_bits

      private

      def __content__
        bits = @unused_bits || 0
        unless Integer === bits && (0..7).cover?(bits)
          raise ASN1Error, "unused_bits for a bitstring value must be in the range 0 to 7"
        end
        [bits].pack("C") + String.new(@value.to_s, encoding: Encoding::BINARY)
      end
    end

    class Boolean
      private

      def __content__
        raise TypeError, "can't convert #{@value.class} into Boolean" unless true == @value || false == @value
        @value ? "\xFF".b : "\x00".b
      end
    end

    class Integer
      private

      def __content__ = ASN1.__send__(:__int_bytes__, @value)
    end

    class Enumerated
      private

      def __content__ = ASN1.__send__(:__int_bytes__, @value)
    end

    class Null
      private

      def __content__
        raise ASN1Error, "nil expected" unless @value.nil?
        "".b
      end
    end

    class ObjectId
      # The short name OpenSSL has for this OID, or nil.
      def sn
        info = ASN1.__send__(:__obj__, @value.to_s)
        info && info[1]
      end
      alias short_name sn

      def ln
        info = ASN1.__send__(:__obj__, @value.to_s)
        info && info[2]
      end
      alias long_name ln

      def oid
        ASN1.__send__(:__obj__, @value.to_s, "OBJ_txt2obj")[3]
      end

      def ==(other)
        return false unless ObjectId === other
        oid == other.oid
      end

      private

      def __content__
        dotted = ASN1.__send__(:__obj__, @value.to_s, "invalid OBJECT ID #{@value}")[3]
        ASN1.__send__(:__oid_bytes__, dotted)
      end
    end

    class UTCTime
      private

      def __content__
        t = ASN1.__send__(:__time__, @value)
        unless (1950..2049).cover?(t.year)
          raise ASN1Error, "ASN1_UTCTIME_adj"
        end
        t.strftime("%y%m%d%H%M%SZ").b
      end
    end

    class GeneralizedTime
      private

      def __content__
        ASN1.__send__(:__time__, @value).strftime("%Y%m%d%H%M%SZ").b
      end
    end

    class << self
      # The constructors, `OpenSSL::ASN1::Integer(1)` and the rest.
      %i[
        Boolean Integer Enumerated BitString OctetString UTF8String NumericString
        PrintableString T61String VideotexString IA5String GraphicString ISO64String
        GeneralString UniversalString BMPString Null ObjectId UTCTime GeneralizedTime
        Sequence Set EndOfContent
      ].each do |name|
        define_method(name) { |*args| ASN1.const_get(name).new(*args) }
      end

      # :call-seq:
      #    OpenSSL::ASN1.decode(der) -> ASN1Data
      #
      # Decodes a BER- or DER-encoded value and creates an ASN1Data instance.
      def decode(der)
        der = __der_arg__(der)
        value, offset = __decode__(der, 0, 0)
        raise ASN1Error, "extra data bytes" unless offset == der.bytesize
        value
      end

      # :call-seq:
      #    OpenSSL::ASN1.decode_all(der) -> Array of ASN1Data
      #
      # Like decode, but takes concatenated values and answers them all.
      def decode_all(der)
        der = __der_arg__(der)
        out = []
        offset = 0
        while offset < der.bytesize
          value, offset = __decode__(der, offset, 0)
          out << value
        end
        out
      end

      private

      def __der_arg__(der)
        der = der.to_der if der.respond_to?(:to_der)
        String.new(IO.__str__(der), encoding: Encoding::BINARY)
      end

      # A value's tag and length octets.
      def __header__(tag_class, tag, constructed, length)
        bits = CLASS_BITS[tag_class] or raise ASN1Error, "invalid tag class"
        bits |= 0x20 if constructed
        out = +"".b
        if tag < 31
          out << (bits | tag).chr
        else
          out << (bits | 0x1f).chr
          digits = []
          t = tag
          loop do
            digits.unshift(t & 0x7f)
            t >>= 7
            break if t.zero?
          end
          digits.each_with_index { |d, i| out << (i == digits.size - 1 ? d : d | 0x80).chr }
        end
        if length < 0x80
          out << length.chr
        else
          bytes = []
          l = length
          while l > 0
            bytes.unshift(l & 0xff)
            l >>= 8
          end
          out << (0x80 | bytes.size).chr << bytes.pack("C*")
        end
        out
      end

      # A primitive's or constructive's DER, with its tagging.
      def __wrap__(item, content, constructed)
        tag = item.tag
        raise ASN1Error, "must specify tag number" unless ::Integer === tag
        tag_class = item.tag_class
        if item.tagging == :EXPLICIT
          inner_tag = item.class.__default_tag__
          raise ASN1Error, "explicit tagging of unknown tag" if inner_tag.nil?
          inner = __header__(:UNIVERSAL, inner_tag, constructed, content.bytesize) + content
          __header__(tag_class, tag, true, inner.bytesize) + inner
        else
          __header__(tag_class, tag, constructed, content.bytesize) + content
        end
      end

      def __int_bytes__(value)
        n = ::Integer === value ? value : value.to_i
        bytes = []
        loop do
          bytes.unshift(n & 0xff)
          n >>= 8
          break if (n.zero? && bytes[0] < 0x80) || (n == -1 && bytes[0] >= 0x80)
        end
        bytes.pack("C*")
      end

      def __oid_bytes__(dotted)
        arcs = dotted.split(".").map(&:to_i)
        first = arcs.shift * 40 + (arcs.shift || 0)
        out = +"".b
        [first, *arcs].each do |arc|
          digits = []
          loop do
            digits.unshift(arc & 0x7f)
            arc >>= 7
            break if arc.zero?
          end
          digits.each_with_index { |d, i| out << (i == digits.size - 1 ? d : d | 0x80).chr }
        end
        out
      end

      def __time__(value)
        case value
        when Time then value.getutc
        else Time.at(Integer(value)).getutc
        end
      end

      # One value at `offset`: `[value, offset after it]`.
      def __decode__(der, offset, depth)
        start = offset
        raise ASN1Error, "too long" if offset >= der.bytesize
        first = der.getbyte(offset)
        offset += 1
        tag_class = CLASS_BITS.key(first & 0xc0)
        constructed = first & 0x20 != 0
        tag = first & 0x1f
        if tag == 0x1f
          tag = 0
          loop do
            raise ASN1Error, "too long" if offset >= der.bytesize
            b = der.getbyte(offset)
            offset += 1
            tag = (tag << 7) | (b & 0x7f)
            break if b & 0x80 == 0
          end
        end
        raise ASN1Error, "too long" if offset >= der.bytesize
        len = der.getbyte(offset)
        offset += 1
        indefinite = false
        if len == 0x80
          raise ASN1Error, "indefinite length for primitive value" unless constructed
          indefinite = true
          len = nil
        elsif len > 0x80
          n = len & 0x7f
          raise ASN1Error, "too long" if offset + n > der.bytesize
          len = 0
          n.times do
            len = (len << 8) | der.getbyte(offset)
            offset += 1
          end
        end
        raise ASN1Error, "too long" if !len.nil? && offset + len > der.bytesize
        if constructed
          items = []
          if indefinite
            loop do
              item, offset = __decode__(der, offset, depth + 1)
              items << item
              break if EndOfContent === item
            end
          else
            stop = offset + len
            while offset < stop
              item, offset = __decode__(der, offset, depth + 1)
              items << item
            end
          end
          value =
            if tag_class == :UNIVERSAL && (tag == SEQUENCE || tag == SET)
              (tag == SEQUENCE ? Sequence : Set).new(items)
            elsif tag_class == :UNIVERSAL
              Constructive.new(items, tag, nil, :UNIVERSAL)
            else
              ASN1Data.new(items, tag, tag_class)
            end
          value.indefinite_length = indefinite
          return [value, offset]
        end
        content = der.byteslice(offset, len)
        offset += len
        [__primitive__(tag_class, tag, content), offset]
      end

      def __primitive__(tag_class, tag, content)
        return ASN1Data.new(content, tag, tag_class) unless tag_class == :UNIVERSAL
        case tag
        when EOC then EndOfContent.new
        when BOOLEAN then Boolean.new(content.bytesize == 1 && content.getbyte(0) != 0)
        when INTEGER then Integer.new(__decode_int__(content))
        when ENUMERATED then Enumerated.new(__decode_int__(content))
        when BIT_STRING
          s = BitString.new(content.byteslice(1..) || "".b)
          s.unused_bits = content.getbyte(0) || 0
          s
        when NULL then Null.new(nil)
        when OBJECT then ObjectId.new(__decode_oid__(content))
        when UTCTIME then UTCTime.new(__decode_time__(content, false))
        when GENERALIZEDTIME then GeneralizedTime.new(__decode_time__(content, true))
        else
          name = UNIVERSAL_TAG_NAME[tag]
          klass = {
            OCTET_STRING => OctetString, UTF8STRING => UTF8String,
            NUMERICSTRING => NumericString, PRINTABLESTRING => PrintableString,
            T61STRING => T61String, VIDEOTEXSTRING => VideotexString, IA5STRING => IA5String,
            GRAPHICSTRING => GraphicString, ISO64STRING => ISO64String,
            GENERALSTRING => GeneralString, UNIVERSALSTRING => UniversalString,
            BMPSTRING => BMPString,
          }[tag]
          if klass
            klass.new(content)
          elsif name
            Primitive.new(content, tag, nil, :UNIVERSAL)
          else
            ASN1Data.new(content, tag, :UNIVERSAL)
          end
        end
      end

      def __decode_int__(content)
        n = 0
        content.each_byte { |b| n = (n << 8) | b }
        n -= 1 << (8 * content.bytesize) if content.bytesize > 0 && content.getbyte(0) >= 0x80
        OpenSSL::BN.new(n)
      end

      def __decode_oid__(content)
        arcs = []
        arc = 0
        content.each_byte do |b|
          arc = (arc << 7) | (b & 0x7f)
          next unless b & 0x80 == 0
          arcs << arc
          arc = 0
        end
        first = arcs.shift || 0
        head = first < 80 ? [first / 40, first % 40] : [2, first - 80]
        dotted = [*head, *arcs].join(".")
        info = __obj__(dotted)
        info && info[1] ? info[1] : dotted
      end

      def __decode_time__(content, generalized)
        s = content.b
        m =
          if generalized
            /\A(\d{4})(\d\d)(\d\d)(\d\d)(\d\d)(\d\d)?(?:[.,]\d+)?(Z|[+-]\d{4})?\z/.match(s)
          else
            /\A(\d\d)(\d\d)(\d\d)(\d\d)(\d\d)(\d\d)?(Z|[+-]\d{4})\z/.match(s)
          end
        raise ASN1Error, "ASN1_TIME_to_tm" if m.nil?
        year = m[1].to_i
        year += year < 50 ? 2000 : 1900 unless generalized
        t = Time.utc(year, m[2].to_i, m[3].to_i, m[4].to_i, m[5].to_i, m[6].to_i)
        zone = m[7]
        if zone && zone != "Z"
          offset = (zone[1, 2].to_i * 3600) + (zone[3, 2].to_i * 60)
          t -= zone[0] == "+" ? offset : -offset
        end
        t
      end
    end
  end
end
