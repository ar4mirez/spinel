# `prism/prism` (#837): the prism gem's C extension, which `prism.rb` loads
# on CRuby. Its Ruby half is the gem's own `lib/`, vendored in `stdlib/` at
# the release Spinel parses with (ruby-prism 1.9.0), and its C half is the
# copy of Prism Spinel's parser already links: `spinel-parse/src/serialize.rs`
# behind four primitives.
#
# - `__prism_dump__(kind, source, options)`: Prism's serialization of a
#   source, read back by the gem's `Prism::Serialize` — the route the gem's
#   own FFI backend (`prism/ffi.rb`) takes, so the node objects are built in
#   Ruby by the gem's own code, never by Spinel;
# - `__prism_success__(source, options)`: `pm_parse_success_p`;
# - `__prism_query__(kind, string, encoding)`: the `StringQuery` predicates;
# - `__prism_version__`: `pm_version`.
#
# The methods are the extension's (`ext/prism/extension.c`), and the options
# are packed as `prism/ffi.rb` packs them, checked as the extension checks
# them. Adapted from prism 1.9.0, Copyright 2022-present Shopify, Inc., under
# the MIT license (`stdlib/LICENSE/prism/LICENSE.md`).

module Prism
  # The extension refuses to load against another release of the library.
  VERSION = __prism_version__.freeze

  # The options a call takes, each a keyword the extension knows.
  OPTION_KEYS = %i[
    filepath encoding line frozen_string_literal version scopes command_line
    main_script partial_script freeze
  ].freeze
  private_constant :OPTION_KEYS

  class << self
    # Mirror the extension's Prism.dump.
    def dump(source, **options)
      __prism_check_string__(source)
      __prism_dump_string__(0, source, options)
    end

    # Mirror the extension's Prism.dump_file.
    def dump_file(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_dump_string__(0, source, options) }
    end

    # Mirror the extension's Prism.lex.
    def lex(code, **options)
      __prism_check_string__(code)
      __prism_lex__(code, options)
    end

    # Mirror the extension's Prism.lex_file.
    def lex_file(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_lex__(source, options) }
    end

    # Mirror the extension's Prism.parse.
    def parse(code, **options)
      __prism_check_string__(code)
      __prism_parse__(code, options)
    end

    # Mirror the extension's Prism.parse_file.
    def parse_file(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_parse__(source, options) }
    end

    # Mirror the extension's Prism.parse_stream: lines read with `gets`, as
    # `pm_parse_stream` reads them, stopping at an `__END__` line so the rest
    # stays in the stream.
    # ponytail: Prism reads on past `__END__` when it ends an unterminated
    # heredoc; that needs the parser's lexer state, so it is not done here.
    def parse_stream(stream, **options)
      __prism_check_options__(options)
      source = +""
      while (line = stream.gets(4095))
        source << line
        break if line == "__END__" || line == "__END__\n" || line == "__END__\r\n"
        break if stream.eof?
      end
      __prism_parse__(source, options)
    end

    # Mirror the extension's Prism.parse_comments.
    def parse_comments(code, **options)
      __prism_check_string__(code)
      __prism_comments__(code, options)
    end

    # Mirror the extension's Prism.parse_file_comments.
    def parse_file_comments(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_comments__(source, options) }
    end

    # Mirror the extension's Prism.parse_lex.
    def parse_lex(code, **options)
      __prism_check_string__(code)
      __prism_parse_lex__(code, options)
    end

    # Mirror the extension's Prism.parse_lex_file.
    def parse_lex_file(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_parse_lex__(source, options) }
    end

    # Mirror the extension's Prism.parse_success?.
    def parse_success?(code, **options)
      __prism_check_string__(code)
      __prism_success__(code, __prism_options__(options))
    end

    # Mirror the extension's Prism.parse_failure?.
    def parse_failure?(code, **options)
      !parse_success?(code, **options)
    end

    # Mirror the extension's Prism.parse_file_success?.
    def parse_file_success?(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_success__(source, __prism_options__(options)) }
    end

    # Mirror the extension's Prism.parse_file_failure?.
    def parse_file_failure?(filepath, **options)
      !parse_file_success?(filepath, **options)
    end

    # Mirror the extension's Prism.profile: parse, and build nothing.
    def profile(source, **options)
      __prism_check_string__(source)
      __prism_dump__(0, source, __prism_options__(options))
      nil
    end

    # Mirror the extension's Prism.profile_file.
    def profile_file(filepath, **options)
      __prism_with_file__(filepath, options) { |source| __prism_dump__(0, source, __prism_options__(options)) }
      nil
    end

    private

    def __prism_dump_string__(kind, source, options)
      dumped = __prism_dump__(kind, source, __prism_options__(options))
      dumped.freeze if options.fetch(:freeze, false)
      dumped
    end

    def __prism_lex__(source, options)
      Serialize.load_lex(source, __prism_dump__(1, source, __prism_options__(options)), options.fetch(:freeze, false))
    end

    def __prism_parse__(source, options)
      Serialize.load_parse(source, __prism_dump__(0, source, __prism_options__(options)), options.fetch(:freeze, false))
    end

    def __prism_parse_lex__(source, options)
      Serialize.load_parse_lex(source, __prism_dump__(2, source, __prism_options__(options)), options.fetch(:freeze, false))
    end

    def __prism_comments__(source, options)
      Serialize.load_parse_comments(source, __prism_dump__(3, source, __prism_options__(options)), options.fetch(:freeze, false))
    end

    # The extension's `check_string`.
    def __prism_check_string__(value)
      return if String === value
      raise TypeError, "wrong argument type #{value.class} (expected String)"
    end

    # A file's source, read as the extension reads it (`pm_string_file_init`),
    # with its path the `filepath` option.
    def __prism_with_file__(filepath, options)
      __prism_check_string__(filepath)
      options[:filepath] = filepath
      source = begin
        File.binread(filepath)
      rescue Errno::EISDIR
        raise Errno::EISDIR, filepath
      end
      yield source
    end

    # The extension's `build_options_i`: every keyword must be one it knows.
    def __prism_check_options__(options)
      options.each_key do |key|
        raise ArgumentError, "unknown keyword: #{key}" unless OPTION_KEYS.include?(key)
      end
    end

    # The options packed as `pm_options_read` reads them (`prism/ffi.rb`'s
    # `dump_options`).
    def __prism_options__(options)
      __prism_check_options__(options)
      template = +""
      values = []

      template << "L"
      if (filepath = options[:filepath])
        __prism_check_string__(filepath)
        values.push(filepath.bytesize, filepath.b)
        template << "A*"
      else
        values << 0
      end

      template << "l"
      values << (options[:line] || 1)

      template << "L"
      encoding = options[:encoding]
      if encoding
        name = Encoding.find(encoding.is_a?(Encoding) ? encoding.name : encoding.to_str).name
        values.push(name.bytesize, name.b)
        template << "A*"
      else
        values << 0
      end

      template << "C"
      values << (options[:frozen_string_literal] ? 1 : 0)

      template << "C"
      values << __prism_command_line__(options[:command_line])

      template << "C"
      values << __prism_version_option__(options[:version])

      template << "C"
      values << (encoding == false ? 1 : 0)

      template << "C"
      values << (options[:main_script] ? 1 : 0)

      template << "C"
      values << (options[:partial_script] ? 1 : 0)

      template << "C"
      values << (options[:freeze] ? 1 : 0)

      template << "L"
      if (scopes = options[:scopes])
        raise TypeError, "wrong argument type #{scopes.class} (expected Array)" unless Array === scopes
        values << scopes.length
        scopes.each do |scope|
          locals = nil
          forwarding = 0
          case scope
          when Array
            locals = scope
          when Scope
            locals = scope.locals
            raise TypeError, "wrong argument type #{locals.class} (expected Array)" unless Array === locals
            names = scope.forwarding
            raise TypeError, "wrong argument type #{names.class} (expected Array)" unless Array === names
            names.each do |forward|
              raise TypeError, "wrong argument type #{forward.class} (expected Symbol)" unless Symbol === forward
              case forward
              when :*     then forwarding |= 0x1
              when :**    then forwarding |= 0x2
              when :&     then forwarding |= 0x4
              when :"..." then forwarding |= 0x8
              else raise ArgumentError, "invalid forwarding value: #{forward}"
              end
            end
          else
            raise TypeError, "wrong argument type #{scope.class} (expected Array or Prism::Scope)"
          end

          template << "L"
          values << locals.length
          template << "C"
          values << forwarding
          locals.each do |local|
            raise TypeError, "wrong argument type #{local.class} (expected Symbol)" unless Symbol === local
            name = local.name
            template << "L"
            values << name.bytesize
            template << "A*"
            values << name.b
          end
        end
      else
        values << 0
      end

      values.pack(template)
    end

    def __prism_command_line__(command_line)
      return 0 if command_line.nil?
      __prism_check_string__(command_line)
      command_line.each_char.inject(0) do |value, char|
        case char
        when "a" then value | 0b000001
        when "e" then value | 0b000010
        when "l" then value | 0b000100
        when "n" then value | 0b001000
        when "p" then value | 0b010000
        when "x" then value | 0b100000
        else raise ArgumentError, "invalid command line flag: '#{char}'"
        end
      end
    end

    # `pm_options_version_set`'s reading of a version: its first three
    # characters, or "current", RUBY_VERSION's.
    def __prism_version_option__(version)
      return 0 if version.nil?
      __prism_check_string__(version)
      current = version == "current"
      case current ? RUBY_VERSION : version
      when "latest" then 0
      when /\A3\.3(\.\d+)?\z/ then 1
      when /\A3\.4(\.\d+)?\z/ then 2
      when /\A3\.5(\.\d+)?\z/, /\A4\.0(\.\d+)?\z/ then 3
      when /\A4\.1(\.\d+)?\z/ then 4
      else
        raise CurrentVersionError, RUBY_VERSION if current
        raise ArgumentError, "invalid version: #{version}"
      end
    end
  end

  class StringQuery
    class << self
      # Mirrors the extension's StringQuery::local?.
      def local?(string)
        __prism_query_answer__(0, string)
      end

      # Mirrors the extension's StringQuery::constant?.
      def constant?(string)
        __prism_query_answer__(1, string)
      end

      # Mirrors the extension's StringQuery::method_name?.
      def method_name?(string)
        __prism_query_answer__(2, string)
      end

      private

      def __prism_query_answer__(kind, string)
        unless String === string
          raise TypeError, "wrong argument type #{string.class} (expected String)"
        end
        case __prism_query__(kind, string, string.encoding.name)
        when 1 then true
        when 0 then false
        else raise ArgumentError, "Invalid or non ascii-compatible encoding"
        end
      end
    end
  end
end
