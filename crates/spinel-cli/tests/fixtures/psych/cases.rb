# The objects and documents `tests/psych.rs` round-trips through the
# built-in psych (#108), shared by `make.rb`, which CRuby runs to write what
# its Psych makes of each, and `roundtrip.rb`, which checks Spinel's against
# that. Every Time here carries its own offset, so nothing depends on TZ.
require "yaml"
require "date"

module PsychCases
  Point = Struct.new(:x, :y)

  # An object Psych dumps by its instance variables.
  class Account
    attr_reader :name, :balance, :tags

    def initialize(name, balance, tags)
      @name = name
      @balance = balance
      @tags = tags
    end

    def ==(other)
      other.class == self.class && other.name == name && other.balance == balance &&
        other.tags == tags
    end
  end

  # An object that says how it is dumped.
  class Temperature
    attr_reader :degrees

    def initialize(degrees) = @degrees = degrees
    def encode_with(coder) = coder["celsius"] = degrees
    def init_with(coder) = @degrees = coder["celsius"]
    def ==(other) = other.class == self.class && other.degrees == degrees
  end

  shared = [1, "two"]
  long = "word " * 30

  # name, object, Psych.dump options
  DUMPS = [
    ["nil", nil],
    ["true", true],
    ["false", false],
    ["integer", 42],
    ["negative", -17],
    ["bignum", 2**80],
    ["float", 3.25],
    ["float_small", 1.5e-10],
    ["float_large", 1.0e+100],
    ["float_negative_zero", -0.0],
    ["infinity", Float::INFINITY],
    ["negative_infinity", -Float::INFINITY],
    ["nan", Float::NAN],
    ["symbol", :locked],
    ["symbol_quoted", :"with space"],
    ["symbol_colon", :"a:b"],
    ["string", "plain"],
    ["string_empty", ""],
    ["string_true", "true"],
    ["string_yes", "yes"],
    ["string_null", "null"],
    ["string_tilde", "~"],
    ["string_integer", "123"],
    ["string_float", "1.5"],
    ["string_octal", "0o17"],
    ["string_hex", "0x1F"],
    ["string_underscored", "1_000"],
    ["string_exponent", "1e3"],
    ["string_date", "2024-11-19"],
    ["string_colon", "a: b"],
    ["string_symbolish", ":notasymbol"],
    ["string_dash", "- item"],
    ["string_comment", "# not a comment"],
    ["string_indicators", "@at `tick %pct !bang &amp *star |pipe >gt"],
    ["string_quotes", %q(it's "quoted")],
    ["string_leading_space", "  leading"],
    ["string_trailing_space", "trailing  "],
    ["string_multiline", "first\nsecond"],
    ["string_multiline_final_newline", "first\nsecond\n"],
    ["string_trailing_newlines", "text\n\n\n"],
    ["string_tab", "tab\there"],
    ["string_control", "bell\a and \e escape"],
    ["string_unicode", "café ✓ 日本語"],
    ["string_long", long],
    ["string_long_multiline", "#{long}\n#{long}"],
    ["string_binary", "\xFF\x00\xFE binary".b],
    ["array", [1, "two", :three, nil, 4.5]],
    ["array_empty", []],
    ["array_nested", [[1, [2, [3, []]]], {}]],
    ["hash", {"name" => "Spinel", "version" => 1, "tags" => ["ruby", "rust"]}],
    ["hash_empty", {}],
    ["hash_symbol_keys", {a: 1, b: {c: [2, 3]}}],
    ["hash_odd_keys", {1 => "one", nil => "nil", true => "yes", 2.5 => "float", [1, 2] => "array"}],
    ["hash_nested", {"a" => {"b" => {"c" => {"d" => [1, {"e" => nil}]}}}}],
    ["aliases", {"first" => shared, "second" => shared, "third" => [shared]}],
    ["time_utc", Time.utc(2024, 2, 29, 23, 59, 58, 123456)],
    ["time_offset", Time.new(2001, 12, 14, 21, 59, 43, "-05:00")],
    ["time_east", Time.new(2023, 7, 4, 9, 30, 0, "+09:30")],
    ["date", Date.new(2024, 2, 29)],
    ["date_time", DateTime.new(2001, 2, 3, 4, 5, 6, "+07:00")],
    ["range", 1..10],
    ["range_exclusive", "a"..."z"],
    ["regexp", /ab+c/ix],
    ["rational", Rational(3, 4)],
    ["complex", Complex(1, -2)],
    ["struct", Point.new(1, "y")],
    ["object", Account.new("checking", 100.25, %w[a b])],
    ["encode_with", Temperature.new(21.5)],
    ["exception", ArgumentError.new("bad input")],
    ["mixed", {"list" => [1, {"k" => "v"}, [nil, true]], "when" => Date.new(2020, 1, 1), :sym => :val}],
    ["option_line_width", {"text" => long}, {line_width: 20}],
    ["option_unlimited_width", {"text" => long}, {line_width: -1}],
    ["option_indentation", {"a" => {"b" => [1, {"c" => 2}]}}, {indentation: 4}],
    ["option_canonical", {"a" => [1, "b", nil]}, {canonical: true}],
    ["option_header", {"a" => 1}, {header: true}],
    ["option_version", [1, 2], {version: [1, 1]}],
  ].freeze

  # name, objects for Psych.dump_stream
  STREAMS = [
    ["stream", [1, "two", [3], {"four" => 4}]],
    ["stream_strings", ["first\nline", "", "true"]],
  ].freeze

  # Documents parsed to a tree and emitted again, which keeps the styles they
  # were written in; and whose nodes' marks are reported.
  REEMIT = [
    ["flow", "{a: [1, 2], b: {c: d}}\n"],
    ["block", "a:\n  - 1\n  - 'two'\n  - \"three\"\nb: |\n  literal\n  text\nc: >\n  folded\n  text\n"],
    ["tags", "%TAG !e! tag:example.com,2000:\n--- !e!thing\nx: !!str 1\ny: !local value\n"],
    ["anchors", "base: &base\n  a: 1\nderived:\n  <<: *base\n  b: 2\n"],
    ["documents", "--- 1\n...\n--- [2]\n--- {three: 3}\n"],
    ["version", "%YAML 1.1\n--- text\n"],
    ["comments", "# heading\na: 1 # trailing\n# between\nb: [x, y] # more\n"],
    ["complex_key", "? [a, b]\n: value\n? {c: d}\n: other\n"],
    ["empty_values", "a:\nb: ~\nc: ''\nd: []\n"],
  ].freeze

  # Loads that must fail as CRuby's do, and loads that must not.
  ERRORS = [
    ["load_object", -> { YAML.load("--- !ruby/object:Object {}\n") }],
    ["load_alias", -> { YAML.load("a: &x 1\nb: *x\n") }],
    ["load_unknown_alias", -> { YAML.unsafe_load("a: *nowhere\n") }],
    ["load_syntax", -> { YAML.load("a: [1, 2\n") }],
    ["load_syntax_file", -> { YAML.load("a: b: c\n", filename: "config.yml") }],
    ["load_bad_escape", -> { YAML.load(%(key: "\\q"\n)) }],
    ["load_tab", -> { YAML.load("a:\n\t- b\n") }],
    ["load_symbol", -> { YAML.load(":sym\n") }],
    ["load_date", -> { YAML.load("2024-01-01\n") }],
    ["safe_load_symbol", -> { YAML.safe_load(":sym\n") }],
    ["safe_load_symbol_permitted", -> { YAML.safe_load(":sym\n", permitted_classes: [Symbol]) }],
    ["safe_load_symbols_listed", -> { YAML.safe_load(":a\n", permitted_classes: [Symbol], permitted_symbols: [:b]) }],
    ["safe_load_date", -> { YAML.safe_load("2024-01-01\n") }],
    ["safe_load_date_permitted", -> { YAML.safe_load("2024-01-01\n", permitted_classes: [Date]) }],
    ["safe_load_time", -> { YAML.safe_load("2024-01-01 10:00:00 +00:00\n") }],
    ["safe_load_alias", -> { YAML.safe_load("a: &x [1]\nb: *x\n") }],
    ["safe_load_alias_enabled", -> { YAML.safe_load("a: &x [1]\nb: *x\n", aliases: true) }],
    ["safe_load_struct", -> { YAML.safe_load("--- !ruby/struct:PsychCases::Point\nx: 1\ny: 2\n") }],
    ["safe_load_struct_permitted", -> { YAML.safe_load("--- !ruby/struct:PsychCases::Point\nx: 1\ny: 2\n", permitted_classes: [PsychCases::Point]) }],
    ["safe_load_object", -> { YAML.safe_load("--- !ruby/object:PsychCases::Account\nname: x\n") }],
    ["unsafe_load_unknown_class", -> { YAML.unsafe_load("--- !ruby/object:NoSuch::Thing {}\n") }],
    ["safe_dump_symbol", -> { YAML.safe_dump(:sym) }],
    ["safe_dump_symbol_permitted", -> { YAML.safe_dump({a: :b}, permitted_symbols: [:a, :b], permitted_classes: [Symbol]) }],
    ["safe_dump_object", -> { YAML.safe_dump(PsychCases::Point.new(1, 2)) }],
    ["load_freeze", -> { YAML.load("a: [b]\n", freeze: true).then { |h| [h.frozen?, h["a"].frozen?, h["a"][0].frozen?] } }],
    ["load_symbolize_names", -> { YAML.load("a: {b: 1}\n", symbolize_names: true) }],
    ["load_fallback", -> { YAML.load("", fallback: :none) }],
    ["load_stream", -> { YAML.load_stream("--- 1\n--- [2]\n") }],
    ["parse_error_fields", -> {
      begin
        YAML.load("x: [1\n", filename: "f.yml")
      rescue Psych::SyntaxError => e
        [e.file, e.line, e.column, e.offset, e.problem, e.context]
      end
    }],
    # Ten aliases to each of nine levels: a billion x's, expanded.
    ["billion_laughs", -> { YAML.safe_load(PsychCases.laughs) }],
    ["billion_laughs_aliases_enabled", -> {
      loaded = YAML.unsafe_load(PsychCases.laughs)
      [loaded.size, loaded["i"].size, loaded["i"][0].equal?(loaded["h"])]
    }],
  ].freeze

  def self.laughs
    doc = +"a: &a [x, x, x, x, x, x, x, x, x, x]\n"
    previous = "a"
    %w[b c d e f g h i].each do |name|
      doc << "#{name}: &#{name} [#{(["*#{previous}"] * 10).join(", ")}]\n"
      previous = name
    end
    doc
  end
end
