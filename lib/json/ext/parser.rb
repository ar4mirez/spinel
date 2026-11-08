# frozen_string_literal: true
#
# json's parser (#55): `JSON::Ext::ParserConfig` and `JSON::Ext::Parser.parse`,
# what CRuby's `ext/json/ext/parser/parser.c` defines, at json 2.18.0.
#
# The gem's Ruby half is upstream's, in `stdlib/json/`; `json/ext.rb` requires
# this file where CRuby loads the C extension. The grammar is
# `crates/spinel-vm/src/json.rs`, reached through `__json_parse__`, which
# reads the document in one pass and builds its objects. What is left here is
# what the C code does around the grammar that Ruby can say: reading the
# options, converting the source to UTF-8, and raising and warning from what
# the parse answers.
#
# Two options call back into Ruby for values the parse makes: `on_load` (which
# `object_class`, `array_class` and `create_additions` are built on) and
# `decimal_class`. With either, the parse hands back objects and decimals
# unbuilt, `[OBJECT, offset, key, value, ...]` and `[DECIMAL, text]` Arrays,
# and `build` makes them here, children first and in document order, calling
# the proc on every value as the C parser does.
#
# ponytail: under `on_load` a malformed document raises before the proc has
# seen any value, where the C parser has already called it on every value
# before the error. Only a proc with side effects can tell. The upgrade is a
# native that calls back into Ruby as it parses.
module JSON
  module Ext
    class ParserConfig
      ALLOW_NAN = 1
      ALLOW_TRAILING_COMMA = 2
      ALLOW_CONTROL_CHARACTERS = 4
      SYMBOLIZE_NAMES = 8
      FREEZE = 16
      DUPLICATES_IGNORED = 32
      DUPLICATES_RAISE = 64
      RAW_OBJECTS = 128
      RAW_FLOATS = 256

      # Two objects no document can contain, marking what `build` builds.
      OBJECT = Object.new.freeze
      DECIMAL = Object.new.freeze
      MARKS = [OBJECT, DECIMAL].freeze
      private_constant :ALLOW_NAN, :ALLOW_TRAILING_COMMA, :ALLOW_CONTROL_CHARACTERS,
        :SYMBOLIZE_NAMES, :FREEZE, :DUPLICATES_IGNORED, :DUPLICATES_RAISE,
        :RAW_OBJECTS, :RAW_FLOATS, :OBJECT, :DECIMAL, :MARKS

      # `parser_config_init`: each option the Hash holds, in its order, the
      # ones this version does not know ignored.
      def initialize(opts)
        raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self) if frozen?
        @max_nesting = 100
        @flags = 0
        @on_load = nil
        @decimal_class = nil
        @decimal_method = nil
        unless opts.nil?
          unless Hash === opts
            raise TypeError, "wrong argument type #{ParserConfig.type_name(opts)} (expected Hash)"
          end
          opts.each { |key, value| configure(key, value) }
        end
      end

      # `cParserConfig_parse`.
      def parse(source)
        source = ParserConfig.convert(source)
        flags = @flags
        flags |= RAW_OBJECTS if @on_load || @decimal_class
        flags |= RAW_OBJECTS | RAW_FLOATS if @decimal_class
        value, failure, warnings = __json_parse__(source, @max_nesting, flags, MARKS)
        warnings&.each { |key, line, column| warn_duplicate(key, line, column) }
        raise_failure(failure) if failure
        if flags & RAW_OBJECTS != 0
          value = build(source, value)
        end
        value
      end

      class << self
        # `StringValue` then `convert_encoding`: a UTF-8 String is parsed as
        # it is, a BINARY one is read as UTF-8, and any other is transcoded.
        def convert(source)
          unless String === source
            unless source.respond_to?(:to_str)
              raise TypeError, "no implicit conversion of #{type_name(source)} into String"
            end
            converted = source.to_str
            unless String === converted
              raise TypeError, "can't convert #{source.class} to String (#{source.class}#to_str gives #{converted.class})"
            end
            source = converted
          end
          encoding = source.encoding
          if encoding == ::Encoding::UTF_8
            source
          elsif encoding == ::Encoding::BINARY
            source.dup.force_encoding(::Encoding::UTF_8)
          else
            source.encode(::Encoding::UTF_8)
          end
        end

        # The name `rb_builtin_class_name` gives a value in a TypeError.
        def type_name(value)
          case value
          when nil then "nil"
          when true then "true"
          when false then "false"
          else value.class.to_s
          end
        end
      end

      private

      def configure(key, value)
        case key
        when :max_nesting
          @max_nesting = value ? ParserConfig.int(value) : 0
        when :allow_nan then flag(ALLOW_NAN, value)
        when :allow_trailing_comma then flag(ALLOW_TRAILING_COMMA, value)
        when :allow_control_characters then flag(ALLOW_CONTROL_CHARACTERS, value)
        when :symbolize_names then flag(SYMBOLIZE_NAMES, value)
        when :freeze then flag(FREEZE, value)
        when :on_load
          @on_load = value || nil
        when :allow_duplicate_key
          @flags &= ~(DUPLICATES_IGNORED | DUPLICATES_RAISE)
          @flags |= value ? DUPLICATES_IGNORED : DUPLICATES_RAISE
        when :decimal_class
          decimal_class(value) if value
        end
      end

      def flag(bit, value)
        if value
          @flags |= bit
        else
          @flags &= ~bit
        end
      end

      # What a `decimal_class` is sent: `try_convert` or `new` when it answers
      # one, else a Class's name read as a method, `Kernel#BigDecimal` for
      # `BigDecimal` and `Foo.Bar` for `Foo::Bar`. Anything else is ignored.
      def decimal_class(value)
        if value.respond_to?(:try_convert)
          @decimal_class = value
          @decimal_method = :try_convert
        elsif value.respond_to?(:new)
          @decimal_class = value
          @decimal_method = :new
        elsif Class === value
          name = value.name || value.inspect
          colon = name.rindex(":")
          if colon
            @decimal_class = Object.const_get(name[0, colon - 1])
            @decimal_method = name[(colon + 1)..].to_sym
          else
            @decimal_class = ::Kernel
            @decimal_method = name.to_sym
          end
        end
      end

      def raise_failure(failure)
        case failure[0]
        when 0
          _, message, line, column = failure
          raise ParserConfig.error(message, line, column)
        when 1
          raise NestingError, "nesting of #{failure[1]} is too deep"
        when 2
          _, key, fragment, line, column = failure
          raise_duplicate(key, fragment, line, column)
        else
          raise EncodingError, "invalid symbol in encoding UTF-8 :#{failure[1].inspect}"
        end
      end

      # `emit_duplicate_key_warning`.
      def warn_duplicate(key, line, column)
        key = key.name if Symbol === key
        JSON.deprecation_warning(
          "detected duplicate key #{key.inspect} in JSON object. This will raise an error in json 3.0 " \
          "unless enabled via `allow_duplicate_key: true` at line #{line} column #{column}"
        )
      end

      # `raise_duplicate_key_error`, which hands the message to
      # `raise_parse_error` as its format string: a `%s` in the key's
      # `inspect` prints the fragment at the object, and `%%` a `%`.
      def raise_duplicate(key, fragment, line, column)
        key = key.name if Symbol === key
        used = false
        message = "duplicate key #{key.inspect}".gsub(/%[%s]/) do |directive|
          if directive == "%%"
            "%"
          elsif used
            directive
          else
            used = true
            fragment
          end
        end
        raise ParserConfig.error("#{message} at line #{line} column #{column}", line, column)
      end

      class << ParserConfig
        def int(value)
          return value if Integer === value
          raise TypeError, "no implicit conversion of #{type_name(value)} into Integer"
        end

        def error(message, line, column)
          error = ParserError.new(message)
          error.instance_variable_set(:@line, line)
          error.instance_variable_set(:@column, column)
          error
        end
      end

      # The values the parse left unbuilt, built children first, each passed
      # through `on_load` as it completes. A stack rather than recursion, so
      # `max_nesting: false` cannot run out of frames.
      def build(source, root)
        frames = []
        node = root
        while true
          # Down to the first value not yet built.
          while Array === node && !node[0].equal?(DECIMAL)
            start = node[0].equal?(OBJECT) ? 2 : 0
            break if start >= node.size
            frames << node << start
            node = node[start]
          end
          value = settle(source, node)
          # Back up, closing every container this completes.
          while true
            return value if frames.empty?
            index = frames.pop
            container = frames.pop
            container[index] = value
            index += 1
            if index < container.size
              frames << container << index
              node = container[index]
              break
            end
            value = close(source, container)
          end
        end
      end

      # A value with nothing left to build inside it.
      def settle(source, node)
        if Array === node
          if node[0].equal?(DECIMAL)
            return load(@decimal_class.__send__(@decimal_method, node[1]))
          end
          return close(source, node)
        end
        load(node)
      end

      # `json_decode_array` or `json_decode_object`, then `json_push_value`.
      def close(source, container)
        if container[0].equal?(OBJECT)
          object = {}
          index = 2
          size = container.size
          while index < size
            object[container[index]] = container[index + 1]
            index += 2
          end
          if object.size < (size - 2) / 2
            duplicated(source, container)
          end
          object.freeze if @flags & FREEZE != 0
          load(object)
        else
          container.freeze if @flags & FREEZE != 0
          load(container)
        end
      end

      def duplicated(source, container)
        return if @flags & DUPLICATES_IGNORED != 0
        seen = {}
        index = 2
        key = nil
        while index < container.size
          before = seen.size
          seen[container[index]] = true
          if seen.size == before
            key = container[index]
            break
          end
          index += 2
        end
        line, column, fragment = __json_where__(source, container[1])
        if @flags & DUPLICATES_RAISE != 0
          raise_duplicate(key, fragment, line, column)
        else
          warn_duplicate(key, line, column)
        end
      end

      def load(value)
        @on_load ? @on_load.call(value) : value
      end
    end

    class Parser
      # `cParser_m_parse`: the source is checked before the options are.
      def self.parse(source, opts)
        source = ParserConfig.convert(source)
        ParserConfig.new(opts).parse(source)
      end
    end
  end
end
