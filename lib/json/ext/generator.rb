# frozen_string_literal: true
#
# json's generator (#55): `JSON::Ext::Generator`, what CRuby's
# `ext/json/ext/generator/generator.c` defines, at json 2.18.0, in Ruby.
#
# The gem's Ruby half is upstream's, in `stdlib/json/`; `json/ext.rb` requires
# this file where CRuby loads the C extension, and this file requires
# upstream's `json/ext/generator/state.rb` after it, as the C extension does.
# The gem also ships a generator in Ruby for TruffleRuby,
# `json/truffle_ruby/generator.rb`; it is not this one, because it prints
# Floats with `Float#to_s` and its State is a plain object with instance
# variables, where the C generator has its own number format and a State whose
# configuration `instance_variables` does not show. This file follows the C
# code instead, function by function.
#
# Two primitives do byte work (`crates/spinel-vm/src/json.rs`):
# `__json_float__` is the gem's vendored `fpconv_dtoa`, whose Grisu2 digits
# are not always the shortest ones `Float#to_s` prints and whose arithmetic is
# 64-bit and wraps, which in Ruby would make every step a Bignum; and
# `__json_escape__` is the C generator's escaping, a byte table walk that as a
# `gsub` cost Spinel some 80µs a String.
#
# A State keeps its configuration in `@__name__` instance variables, which
# Spinel does not list in `instance_variables` or `inspect`, as a C struct is
# not: upstream's `State#to_h` adds every listed instance variable to what it
# answers.
module JSON
  module Ext
    module Generator
      class State
        # `cState_from_state_s`: a State as it is, a new one configured from
        # a Hash, a default one from anything else.
        def self.from_state(opts)
          if self === opts
            opts
          elsif Hash === opts
            new(opts)
          else
            State.new
          end
        end

        # `cState_m_generate`, what `JSON.generate` calls: a State that is
        # never made into an object, configured from `opts` alone.
        def self.generate(obj, opts, io)
          state = State.allocate
          state.__send__(:_configure, opts)
          Run.new(state, nil).document(obj, io)
        end

        def initialize_copy(orig)
          super
          self
        end

        def indent
          @__indent__ || ""
        end

        def indent=(indent)
          __modify__
          @__indent__ = State.string_config(indent)
          nil
        end

        def space
          @__space__ || ""
        end

        def space=(space)
          __modify__
          @__space__ = State.string_config(space)
          nil
        end

        def space_before
          @__space_before__ || ""
        end

        def space_before=(space_before)
          __modify__
          @__space_before__ = State.string_config(space_before)
          nil
        end

        def object_nl
          @__object_nl__ || ""
        end

        def object_nl=(object_nl)
          __modify__
          @__object_nl__ = State.string_config(object_nl)
          nil
        end

        def array_nl
          @__array_nl__ || ""
        end

        def array_nl=(array_nl)
          __modify__
          @__array_nl__ = State.string_config(array_nl)
          nil
        end

        def as_json
          @__as_json__ || false
        end

        def as_json=(as_json)
          __modify__
          @__as_json__ = State.proc_config(as_json)
          nil
        end

        def max_nesting
          @__max_nesting__.nil? ? 100 : @__max_nesting__
        end

        def max_nesting=(depth)
          __modify__
          @__max_nesting__ = State.long_config(depth)
          nil
        end

        def script_safe
          @__script_safe__ ? true : false
        end
        alias_method :script_safe?, :script_safe

        def script_safe=(enable)
          __modify__
          @__script_safe__ = enable ? true : false
          nil
        end
        alias_method :escape_slash, :script_safe
        alias_method :escape_slash?, :script_safe?
        alias_method :escape_slash=, :script_safe=

        def strict
          @__strict__ ? true : false
        end
        alias_method :strict?, :strict

        def strict=(enable)
          __modify__
          @__strict__ = enable ? true : false
          nil
        end

        def check_circular?
          max_nesting != 0
        end

        def allow_nan?
          @__allow_nan__ ? true : false
        end

        def allow_nan=(enable)
          __modify__
          @__allow_nan__ = enable ? true : false
          nil
        end

        def ascii_only?
          @__ascii_only__ ? true : false
        end

        def ascii_only=(enable)
          __modify__
          @__ascii_only__ = enable ? true : false
          nil
        end

        def depth
          @__depth__ || 0
        end

        def depth=(depth)
          __modify__
          @__depth__ = State.long_config(depth)
          nil
        end

        def buffer_initial_length
          @__buffer_initial_length__ || 1024
        end

        def buffer_initial_length=(length)
          __modify__
          State.buffer_initial_length(self, length)
          nil
        end

        # `cState_generate`.
        def generate(obj, io = nil)
          Run.new(self, nil).document(obj, io)
        end

        class << self
          # `string_config`: nil, false or "" unset it; anything else must be
          # a String, kept frozen.
          def string_config(config)
            if config
              unless String === config
                raise TypeError, "wrong argument type #{type_name(config)} (expected String)"
              end
              return config.frozen? ? config : config.dup.freeze unless config.empty?
            end
            false
          end

          # `long_config`: false or nil is 0.
          def long_config(value)
            return 0 unless value
            return value if Integer === value
            raise TypeError, "no implicit conversion of #{type_name(value)} into Integer"
          end

          # `rb_convert_type(value, T_DATA, "Proc", "to_proc")`.
          def proc_config(value)
            return value if Proc === value
            unless value.respond_to?(:to_proc)
              raise TypeError, "no implicit conversion of #{type_name(value)} into Proc"
            end
            converted = value.to_proc
            unless Proc === converted
              raise TypeError, "can't convert #{value.class} to Proc (#{value.class}#to_proc gives #{converted.class})"
            end
            converted
          end

          def buffer_initial_length(state, length)
            unless Integer === length
              raise TypeError, "wrong argument type #{type_name(length)} (expected Integer)"
            end
            state.instance_variable_set(:@__buffer_initial_length__, length) if length > 0
          end

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

        def __modify__
          raise FrozenError.new("can't modify frozen #{self.class}: #{inspect}", receiver: self) if frozen?
        end

        # `cState_configure`, which `state.rb`'s `configure` calls.
        def _configure(opts)
          __modify__
          return self unless opts
          unless Hash === opts
            raise TypeError, "wrong argument type #{State.type_name(opts)} (expected Hash)"
          end
          opts.each do |key, value|
            case key
            when :indent then @__indent__ = State.string_config(value)
            when :space then @__space__ = State.string_config(value)
            when :space_before then @__space_before__ = State.string_config(value)
            when :object_nl then @__object_nl__ = State.string_config(value)
            when :array_nl then @__array_nl__ = State.string_config(value)
            when :max_nesting then @__max_nesting__ = State.long_config(value)
            when :allow_nan then @__allow_nan__ = value ? true : false
            when :ascii_only then @__ascii_only__ = value ? true : false
            when :depth then @__depth__ = State.long_config(value)
            when :buffer_initial_length then State.buffer_initial_length(self, value)
            when :script_safe, :escape_slash then @__script_safe__ = value ? true : false
            when :strict then @__strict__ = value ? true : false
            when :allow_duplicate_key then @__duplicate__ = value ? :ignore : :raise
            when :as_json then @__as_json__ = value ? State.proc_config(value) : false
            end
          end
          self
        end

        # `cState_allow_duplicate_key_p`: nil while it is the deprecated
        # default.
        def allow_duplicate_key?
          case @__duplicate__
          when :ignore then true
          when :raise then false
          end
        end

        public

        # The configuration a Run reads, and that `vstate_spill` copies into
        # the State a `to_json` is given.
        def __config__
          [@__indent__, @__space__, @__space_before__, @__object_nl__, @__array_nl__,
           @__as_json__, max_nesting, depth, @__duplicate__, allow_nan?, ascii_only?,
           script_safe?, strict?, buffer_initial_length]
        end

        def __restore__(config)
          @__indent__, @__space__, @__space_before__, @__object_nl__, @__array_nl__,
            @__as_json__, @__max_nesting__, @__depth__, @__duplicate__, @__allow_nan__,
            @__ascii_only__, @__script_safe__, @__strict__, @__buffer_initial_length__ = config
          self
        end
      end

      # One generation: `struct generate_json_data` and the functions that
      # take it. The configuration is copied out of the State once, so the
      # hot path reads instance variables of its own.
      class Run
        NESTING = "is too deep. Did you try to serialize objects with circular references?"

        def initialize(state, buffer)
          @state = state
          @indent, @space, @space_before, @object_nl, @array_nl, @as_json, @max_nesting,
            @depth, @duplicate, @allow_nan, @ascii_only, @script_safe, @strict = state.__config__
          @escape = (@script_safe ? 1 : 0) | (@ascii_only ? 2 : 0)
          @buffer = buffer || +""
          @vstate = nil
        end

        # `generate_json_try` over `generate_json`, then `fbuffer_finalize`.
        def document(obj, io)
          value(obj, @depth)
          finish(io)
        end

        # `cState_partial_generate`: one of the generator functions on its own,
        # what `to_json` calls.
        def partial(kind, obj)
          case kind
          when :object then object(obj, @depth)
          when :array then array(obj, @depth)
          when :integer then @buffer << obj.to_s
          when :float then float(obj, @depth)
          when :string then string(obj)
          end
          finish(nil)
        end

        private

        def finish(io)
          if io
            io.write(@buffer)
            io.flush
            io
          else
            @buffer.force_encoding(::Encoding::UTF_8)
          end
        end

        # `generate_json`.
        def value(obj, depth, as_json_called = false)
          buffer = @buffer
          # The commonest classes first: each test is an interpreted step.
          klass = obj.class
          if klass.equal?(::String)
            if (quoted = __json_escape__(obj, @escape))
              buffer << quoted
            elsif as_json_called
              raise GeneratorError.new("source sequence is illegal/malformed utf-8", obj)
            else
              value(valid_encoding(obj, false, false), depth, true)
            end
          elsif klass.equal?(::Integer)
            buffer << obj.to_s
          elsif obj.nil?
            buffer << "null"
          elsif obj.equal?(false)
            buffer << "false"
          elsif obj.equal?(true)
            buffer << "true"
          else
            if klass.equal?(::Hash)
              object(obj, depth)
            elsif klass.equal?(::Array)
              array(obj, depth)
            elsif klass.equal?(::Float)
              float(obj, depth)
            elsif klass.equal?(::Symbol)
              if @strict
                string(obj.name)
              else
                fallback(obj, depth)
              end
            elsif klass.equal?(::JSON::Fragment)
              fragment = obj.json
              unless String === fragment
                raise TypeError, "wrong argument type #{State.type_name(fragment)} (expected String)"
              end
              append(fragment)
            elsif @strict
              if @as_json && !as_json_called
                value(@as_json.call(obj, false), depth, true)
              else
                raise GeneratorError.new("#{Run.class_of(obj)} not allowed in JSON", obj)
              end
            else
              fallback(obj, depth)
            end
          end
        end

        # `generate_json_fallback`.
        def fallback(obj, depth)
          if obj.respond_to?(:to_json)
            unless @vstate
              @vstate = State.allocate.__restore__(@state.__config__)
            end
            @vstate.instance_variable_set(:@__depth__, depth)
            json = obj.to_json(@vstate)
            unless String === json
              raise TypeError, "wrong argument type #{State.type_name(json)} (expected String)"
            end
            append(json)
          else
            string(checked_to_s(obj))
          end
        end

        def checked_to_s(obj)
          text = obj.to_s
          unless String === text
            raise TypeError, "wrong argument type #{State.type_name(text)} (expected String)"
          end
          text
        end

        # `fbuffer_append_str`: the bytes as they are, whatever their
        # encoding says.
        def append(text)
          @buffer.__splice__(@buffer.bytesize, 0, text)
        end

        def increase(depth)
          depth += 1
          if depth > @max_nesting && @max_nesting != 0
            raise NestingError, "nesting of #{depth - 1} #{NESTING}"
          end
          depth
        end

        # `generate_json_object` and `json_object_i`.
        def object(hash, depth)
          depth = increase(depth)
          buffer = @buffer
          if hash.empty?
            buffer << "{}"
            return
          end
          buffer << "{"
          first = true
          first_type = nil
          mixed = false
          indent = @indent
          object_nl = @object_nl
          hash.each_pair do |key, val|
            type = String === key ? :string : (Symbol === key ? :symbol : :other)
            if first
              first = false
              first_type = type
            else
              buffer << ","
            end
            buffer << object_nl if object_nl
            buffer << indent * depth if indent
            as_json_called = false
            while true
              case type
              when :string
                if first_type != :string && !mixed
                  mixed = true
                  mixed_keys(hash)
                end
                key_s = key.class.equal?(::String) ? key : string_subclass(key)
                break
              when :symbol
                if first_type != :symbol && !mixed
                  mixed = true
                  mixed_keys(hash)
                end
                key_s = key.name
                break
              else
                if @strict
                  if @as_json && !as_json_called
                    key = @as_json.call(key, true)
                    type = String === key ? :string : (Symbol === key ? :symbol : :other)
                    as_json_called = true
                    next
                  end
                  raise GeneratorError.new("#{Run.class_of(key)} not allowed as object key in JSON", key)
                end
                key_s = key.to_s
                unless String === key_s
                  raise TypeError, "can't convert #{key.class} to String (#{key.class}#to_s gives #{key_s.class})"
                end
                break
              end
            end
            key_s = valid_encoding(key_s, as_json_called, true)
            if key_s.class.equal?(::String)
              raw_string(key_s)
            else
              value(key_s, depth)
            end
            buffer << @space_before if @space_before
            buffer << ":"
            buffer << @space if @space
            value(val, depth)
          end
          depth -= 1
          if object_nl
            buffer << object_nl
            buffer << indent * depth if indent
          end
          buffer << "}"
        end

        # `json_inspect_hash_with_mixed_keys`.
        def mixed_keys(hash)
          return if @duplicate == :ignore
          ::JSON.__send__(:on_mixed_keys_hash, hash, @duplicate == :raise)
        end

        # `convert_string_subclass`.
        def string_subclass(key)
          text = key.to_s
          unless String === text
            raise TypeError, "can't convert #{key.class} to String (#{key.class}#to_s gives #{text.class})"
          end
          text
        end

        # `generate_json_array`.
        def array(array, depth)
          depth = increase(depth)
          buffer = @buffer
          if array.empty?
            buffer << "[]"
            return
          end
          indent = @indent
          array_nl = @array_nl
          buffer << "["
          buffer << array_nl if array_nl
          index = 0
          while index < array.size
            if index > 0
              buffer << ","
              buffer << array_nl if array_nl
            end
            buffer << indent * depth if indent
            value(array[index], depth)
            index += 1
          end
          depth -= 1
          if array_nl
            buffer << array_nl
            buffer << indent * depth if indent
          end
          buffer << "]"
        end

        # `generate_json_float`.
        def float(obj, depth)
          if obj.finite?
            @buffer << __json_float__(obj)
          else
            unless @allow_nan
              if @strict && @as_json
                casted = @as_json.call(obj, false)
                unless casted.equal?(obj)
                  value(casted, increase(depth))
                  return
                end
              end
              raise GeneratorError.new("#{obj} not allowed in JSON", obj)
            end
            @buffer << obj.to_s
          end
        end

        # `generate_json_string`.
        def string(obj)
          raw_string(valid_encoding(obj, false, false))
        end

        # `valid_json_string_p`.
        def valid?(text)
          return true if text.ascii_only?
          text.encoding == ::Encoding::UTF_8 && text.valid_encoding?
        end

        # `ensure_valid_encoding`.
        def valid_encoding(text, as_json_called, key)
          return text if valid?(text)
          if !as_json_called && @strict && @as_json
            coerced = @as_json.call(text, false)
            unless coerced.equal?(text)
              if String === coerced
                unless valid?(coerced)
                  raise GeneratorError.new("source sequence is illegal/malformed utf-8", text)
                end
              elsif key
                raise GeneratorError.new("#{Run.class_of(coerced)} not allowed as object key in JSON", coerced)
              end
              return coerced
            end
          end
          if text.encoding == ::Encoding::BINARY
            utf8 = text.dup.force_encoding(::Encoding::UTF_8)
            if utf8.valid_encoding?
              unless utf8.ascii_only?
                warn("JSON.generate: UTF-8 string passed as BINARY, this will raise an encoding error in json 3.0",
                     uplevel: ::JSON::Ext::Generator.uplevel)
              end
              return utf8
            end
          end
          begin
            text.encode(::Encoding::UTF_8)
          rescue => error
            raise GeneratorError.new(error.message, text)
          end
        end

        # `raw_generate_json_string`: the escaping is `__json_escape__`'s,
        # which answers nil for a String that is not valid JSON text.
        def raw_string(text)
          quoted = __json_escape__(text, @escape)
          raise GeneratorError.new("source sequence is illegal/malformed utf-8", text) unless quoted
          @buffer << quoted
        end

        # `CLASS_OF`, which a message names: the singleton class when there
        # is one, as every Class and Module has. One that holds no methods
        # cannot be told from none here, so only those that do are named.
        def self.class_of(obj)
          if Module === obj || !obj.singleton_methods.empty?
            obj.singleton_class
          else
            obj.class
          end
        end
      end
      private_constant :Run

      # How far up the stack the first frame outside the json gem is, for a
      # warning `rb_warn` would have reported at the caller's line.
      def self.uplevel
        common = ::JSON.method(:generate).source_location&.first
        gem_root = common ? File.dirname(common) + "/" : nil
        level = 0
        caller_locations(1).each do |frame|
          path = frame.path
          break unless path.nil? || path.start_with?("<internal:") || (gem_root && path.start_with?(gem_root))
          level += 1
        end
        level
      end

      module GeneratorMethods
        module Object
          # `mObject_to_json`.
          def to_json(state = nil)
            text = to_s
            unless String === text
              raise TypeError, "wrong argument type #{State.type_name(text)} (expected String)"
            end
            Run.new(State.from_state(state), nil).partial(:string, text)
          end
        end

        module Hash
          def to_json(state = nil)
            Run.new(State.from_state(state), nil).partial(:object, self)
          end
        end

        module Array
          def to_json(state = nil)
            Run.new(State.from_state(state), nil).partial(:array, self)
          end
        end

        module Integer
          def to_json(state = nil)
            Run.new(State.from_state(state), nil).partial(:integer, self)
          end
        end

        module Float
          def to_json(state = nil)
            Run.new(State.from_state(state), nil).partial(:float, self)
          end
        end

        module String
          def to_json(state = nil)
            Run.new(State.from_state(state), nil).partial(:string, self)
          end
        end

        module TrueClass
          def to_json(state = nil)
            +"true"
          end
        end

        module FalseClass
          def to_json(state = nil)
            +"false"
          end
        end

        module NilClass
          def to_json(state = nil)
            +"null"
          end
        end
      end
    end
  end
end

require "json/ext/generator/state"
