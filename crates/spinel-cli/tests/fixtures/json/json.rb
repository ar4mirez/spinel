# The built-in json (#55): the vendored Ruby half over Spinel's parser and
# generator, run over valid and malformed documents, every parser and
# generator option, and adversarial input. `json.out` beside this file is what
# ruby 4.0.9 (json 2.18.0) prints for it:
#
#   ruby crates/spinel-cli/tests/fixtures/json/json.rb > crates/spinel-cli/tests/fixtures/json/json.out
#
# Nothing here may loop or grow without bound under CRuby, whose parser
# recurses: the deepest document is 10,000 levels.
require "json"
require "stringio"

$warnings = []
module Warning
  def self.warn(message, category: nil)
    # Where the file is differs between the two Rubies; the line does not.
    $warnings << message.sub(/\A\S*\/([^\/\s]+\.rb):/, '\1:')
  end
end

def show(label)
  print label, ": "
  value = yield
  puts value.inspect
rescue JSON::ParserError, JSON::GeneratorError, EncodingError, TypeError, ArgumentError, FrozenError, NoMethodError => e
  line = e.respond_to?(:line) ? " #{e.line.inspect}/#{e.column.inspect}" : ""
  puts "#{e.class}: #{e.message.gsub(/0x\h+/, "0x...").inspect}#{line}"
ensure
  $warnings.each { |w| puts "  warning: #{w.inspect}" }
  $warnings.clear
end

def parse(label, source, opts = nil)
  show(label) { opts ? JSON.parse(source, opts) : JSON.parse(source) }
end

puts "== version"
show("VERSION") { JSON::VERSION }
show("classes") { [JSON::Parser, JSON::State, JSON.generator, JSON::Ext::ParserConfig] }

puts "== values"
parse("scalars", '[null, true, false, 0, -0, 1, -1, 3.25, -0.0, 1E5, 1e+2, 1e-2, 0.1]')
parse("bare scalars", ' "str" ')
parse("integer", "123")
parse("big", "[123456789012345678, 12345678901234567, -123456789012345678901234567890]")
parse("strings", %q(["", "a", "\u00e9\ud83d\ude00\/\b\f\n\r\t\"\\\\", "\u0000", "é€😀"]))
parse("lone low surrogate", %q(["\udc00"]))
parse("nested", '{"a": {"b": [1, {"c": []}]}, "d": {}}')
parse("comments", "  /* a */ [1 /* b */, // c\n 2] // d")
parse("empty comment", "/**/[1]")
parse("whitespace", " \t\r\n[ 1 , 2 ]\n\n")
parse("keys order", '{"b": 1, "a": 2, "c": 3}')
parse("duplicate keys", '{"b": 1, "a": 2, "b": 3}')

puts "== errors"
[
  "", " ", "\n", "[1,]", "[1 2]", '{"a" 1}', '{"a":1,}', '{"a":1 "b"}', "{1:2}", "{,}",
  "[\"a\tb\"]", "[\"a\nb\"]", "[\"a\\qb\"]", "[\"\\u12\"]", "[\"\\u12zz\"]", "[\"ab\\u12\"]",
  "[\"xy\\u0041\\u12\"]", "[\"\\ud800\"]", "[\"ab\\ud800\"]", "[\"\\ud800\\u0041\"]",
  "[\"\\ud800x       \"]", "[\"abc", "[\"\\", "[\"ab\\", "[1", "[01]", "[-]", "[1.]", "[1e]",
  "[.5]", "[-Inf]", "[-Infinity]", "[NaN]", "[Infinity]", "nul", "truex", "[1] x", "[1x]",
  "/* x", "/x", "/*/ [1]", "[1]/", "\n\n  [1,\n   x]", "[1,\r\n2,\n x]", "[\x00]",
  "[\"a\x01\"]", "[\"\\t\\\t\"]", "[\"\\\n\"]", "[\"%s %d\"x]",
  "[\"ééééééééééééééééé x", "[xéééééééééééééééé]", "[xxéééééééééééééééé]", "[x€€€€€€€€€€€]",
  "[aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]", "[éé]",
].each_with_index do |source, i|
  parse("error #{i} #{source.inspect}", source)
end

puts "== truncated"
doc = %q({"name": "Ünïcödé \u00e9\ud83d\ude00", "list": [1, -2.5e3, true, null, {"k": "v\n"}], "n": 0.5})
(0...doc.bytesize).each do |cut|
  part = doc.byteslice(0, cut).force_encoding("UTF-8")
  parse("cut #{cut}", part)
end

puts "== numbers"
parse("exponents", "[1e400, -1e400, 1e-400, 1e4294967297, 1e2147483648, 0.00001e313, 1e99999999999999999999]")
parse("long mantissas", "[1.00000000000000000001e400, 123456789012345678e-500, 0.0000000000000000000000000000000000001e-300]")
parse("subnormals", "[5e-324, 2.2250738585072014e-308, 4.9406564584124654e-324, 1e-310]")
parse("boundaries", "[1.7976931348623157e308, 1.7976931348623159e308, 9007199254740993, 9007199254740993.0]")
show("long float") { JSON.parse("[1.#{'1' * 100_000}]").first }
show("long integer") { JSON.parse("[#{'7' * 5_000}]").first.to_s.size }
show("negative long integer") { JSON.parse("[-#{'3' * 400}]").first % 1_000_007 }
r = 0x2545F4914F6CDD1D
mask = (1 << 64) - 1
decimals = (0...300).map do
  r ^= (r << 13) & mask
  r ^= r >> 7
  r ^= (r << 17) & mask
  digits = (r % 10**(1 + r % 19)).to_s
  exp = (r >> 20) % 700 - 350
  "#{r.odd? ? '-' : ''}#{digits[0]}.#{digits[1..]}e#{exp}"
end
show("decimal round trip") { JSON.parse("[#{decimals.join(',')}]").each_slice(10).map { |s| s.map { |f| [f].pack("E").unpack1("Q<").to_s(16) }.join(" ") } }

puts "== nesting"
parse("default limit ok", "[" * 100 + "]" * 100)
parse("default limit", "[" * 101 + "]" * 101)
parse("max 3", "[[[[1]]]]", max_nesting: 3)
parse("max 2 object", '{"a":{"b":{"c":1}}}', max_nesting: 2)
parse("empty does not nest", "[[]]", max_nesting: 1)
parse("max 1", "[[1]]", max_nesting: 1)
parse("max 0", '{"a":[[[]]]}', max_nesting: 0)
parse("max nil", "[[1]]", max_nesting: nil)
parse("max false", "[[1]]", max_nesting: false)
parse("max string", "[1]", max_nesting: "x")
show("deep unchecked") do
  x = JSON.parse("[" * 10_000 + "1" + "]" * 10_000, max_nesting: false)
  depth = 0
  x, depth = x[0], depth + 1 while Array === x
  [depth, x]
end
show("deep parse!") do
  x = JSON.parse!("{\"a\":" * 5_000 + "1" + "}" * 5_000)
  depth = 0
  x, depth = x["a"], depth + 1 while Hash === x
  [depth, x]
end

puts "== options"
parse("allow_nan", "[NaN, Infinity, -Infinity]", allow_nan: true)
parse("allow_nan false", "[NaN]", allow_nan: false)
parse("trailing comma", '[1, [2,], {"a": 1,},]', allow_trailing_comma: true)
parse("trailing comma off", "[1,]", allow_trailing_comma: false)
parse("control characters", "[\"a\tb\", \"\\\t\"]", allow_control_characters: true)
parse("symbolize", '{"a": {"b": 1}, "é": 2}', symbolize_names: true)
show("symbol encodings") { JSON.parse('{"a": 1, "é": 2}', symbolize_names: true).keys.map(&:encoding) }
show("bad symbol") { JSON.parse(%({"\xff":1}), symbolize_names: true) }
show("frozen keys") { JSON.parse('{"a":"b"}').then { |h| [h.keys[0].frozen?, h.values[0].frozen?] } }
show("freeze") do
  x = JSON.parse('[{"a":"b"}, [1], "s"]', freeze: true)
  [x.frozen?, x[0].frozen?, x[0]["a"].frozen?, x[1].frozen?, x[2].frozen?]
end
show("interned keys") { JSON.parse('[{"abc":1},{"abc":2}]').then { |a| a[0].keys[0].equal?(a[1].keys[0]) } }
show("interned frozen values") { JSON.parse('["abc"]', freeze: true)[0].equal?(-"abc") }
parse("duplicate allowed", '{"a":1,"a":2}', allow_duplicate_key: true)
parse("duplicate refused", '{"a":1,"a":2}', allow_duplicate_key: false)
parse("duplicate refused nested", "[{\n\"x\":1,\"x\":2}]", allow_duplicate_key: false)
parse("duplicate with percent", '{"%s":1,"%s":2}', allow_duplicate_key: false)
parse("duplicate symbol", '{"a":1,"a":2}', allow_duplicate_key: false, symbolize_names: true)
show("duplicate warns") do
  Warning[:deprecated] = true
  JSON.parse(%({\n "b":1, "a":1,\n"a":2}))
ensure
  Warning[:deprecated] = false
end
parse("decimal Rational", "[1.5, 2, 1e2]", decimal_class: Rational)
show("decimal try_convert") do
  o = Object.new
  def o.try_convert(s) = [s, s.encoding.to_s]
  JSON.parse("[1.25]", decimal_class: o)
end
show("decimal new") do
  k = Class.new { attr_reader :s; def initialize(s) = (@s = s) }
  JSON.parse("[1.25]", decimal_class: k).map(&:s)
end
parse("decimal ignored", "[1.5]", decimal_class: ->(s) { s })
show("on_load order") do
  calls = []
  JSON.parse('[1,{"a":[true,"x"]},2.5]', on_load: ->(v) { calls << v; v })
  calls
end
show("on_load replaces") { JSON.parse('{"a":[1,2],"b":"c"}', on_load: ->(v) { Integer === v ? v * 10 : v }) }
show("on_load duplicate") do
  calls = []
  [JSON.parse('{"a":1,"a":2}', on_load: ->(v) { calls << v; v }, allow_duplicate_key: true), calls]
end
show("on_load duplicate refused") { JSON.parse("[{\"a\":1,\"a\":2}]", on_load: proc { |v| v }, allow_duplicate_key: false) }
show("on_load freeze") { JSON.parse('[{"a":[1]}]', on_load: ->(v) { v }, freeze: true).then { |x| [x.frozen?, x[0].frozen?, x[0]["a"].frozen?] } }
show("object_class") do
  klass = Class.new(Hash) { def self.name = "MyHash" }
  r = JSON.parse('{"a":{"b":1}}', object_class: klass)
  [r.class.name, r["a"].class.name, r]
end
show("array_class") do
  klass = Class.new(Array) { def self.name = "MyArray" }
  r = JSON.parse("[[1],2]", array_class: klass)
  [r.class.name, r[0].class.name, r]
end
show("ParserConfig") { JSON::Ext::ParserConfig.new({}).parse("[2]") }
show("ParserConfig nil") { JSON::Ext::ParserConfig.new(nil).parse("[3]") }
show("ParserConfig bad") { JSON::Ext::ParserConfig.new(3) }
show("ParserConfig frozen") { JSON::Ext::ParserConfig.new({}).freeze.parse("[4]") }
show("Parser.new") { p = JSON::Ext::Parser.new("[1]"); [p.parse, p.source] }
show("Parser bad opts") { JSON::Ext::Parser.new("[1]", 3) }
show("parse nil") { JSON.parse(nil) }
show("parse Integer") { JSON.parse(1) }
show("parse bad opts") { JSON.parse("[]", 3) }
show("parse to_str") { o = Object.new; def o.to_str = "[5]"; JSON.parse(o) }
show("parse nil opts") { JSON.parse("[1]", nil) }
show("parse!") { JSON.parse!("[NaN]") }
show("JSON()") { [JSON("[1]"), JSON([1])] }
show("JSON[]") { [JSON["[1]"], JSON[[1]]] }

puts "== encodings"
show("invalid UTF-8 inside a string") { s = JSON.parse("[\"\xff\"]")[0]; [s, s.encoding, s.valid_encoding?] }
show("binary source") { JSON.parse("[\"\xc3\xa9\", 1.5]".b) }
show("latin-1 source") { JSON.parse("[\"\xe9\"]".force_encoding("ISO-8859-1")) }
show("utf-16 source") { JSON.parse('["é", 1]'.encode("UTF-16LE")) }
show("string encodings") { JSON.parse('["a", "é", {"k": 1}]').then { |a| [a[0].encoding, a[1].encoding, a[2].keys[0].encoding] } }

puts "== generate"
show("types") { JSON.generate([1, 2.5, "x", nil, true, false, {"a" => []}, :sym, 2**70, -0.0]) }
show("floats") { JSON.generate([1.0, 1e20, 1.5e-7, 100.0, 1e16, 123456789.123456789, 1e15, 100000000000000.0, 5e-324, 1.7976931348623157e308, -2.2471348024634545e-10]) }
r = 0x9E3779B97F4A7C15
floats = []
while floats.size < 2000
  r ^= (r << 13) & mask
  r ^= r >> 7
  r ^= (r << 17) & mask
  f = [r].pack("Q<").unpack1("E")
  floats << f if f.finite?
end
show("random floats") { JSON.generate(floats).split(",").each_slice(8).map { |s| s.join(",") } }
show("round trip") { JSON.parse(JSON.generate(floats)) == floats }
show("escapes") { JSON.generate("é\u2028\u2029/\x01\x1f\"\\\b\f\n\r\t😀") }
show("script_safe") { JSON.generate("é\u2028/</script>", script_safe: true) }
show("escape_slash") { JSON.generate("a/b", escape_slash: true) }
show("ascii_only") { JSON.generate("é😀\u2028/\x01", ascii_only: true) }
show("ascii_only script_safe") { JSON.generate("é😀\u2028/", ascii_only: true, script_safe: true) }
show("pretty") { JSON.pretty_generate({"a" => [1, 2, {"b" => nil}], "c" => {}, "d" => [], "e" => [[]]}) }
show("custom layout") { JSON.generate({"a" => [1, {"b" => 2}]}, indent: "\t", space: " ", space_before: " ", object_nl: "\n", array_nl: "\n") }
show("nan") { JSON.generate(Float::NAN) }
show("infinity") { JSON.generate([-Float::INFINITY]) }
show("allow_nan") { JSON.generate([Float::NAN, Float::INFINITY, -Float::INFINITY], allow_nan: true) }
show("invalid utf-8") { JSON.generate("\xff") }
show("invalid key") { JSON.generate({"\xff" => 1}) }
show("binary utf-8") { $VERBOSE = true; JSON.generate("\xc3\xa9".b) }
$VERBOSE = false
show("binary ascii") { JSON.generate("abc".b) }
show("binary invalid") { JSON.generate("\xff".b) }
show("latin-1") { JSON.generate("\xe9".force_encoding("ISO-8859-1")) }
show("us-ascii high bytes") { JSON.generate("caf\xC3\xA9".force_encoding("US-ASCII")) }
show("us-ascii key") { JSON.generate({"k\xC3\xA9".force_encoding("US-ASCII") => 1}) }
show("utf-16") { JSON.generate("é".encode("UTF-16LE")) }
show("circular") { a = []; a << a; JSON.generate(a) }
show("max_nesting") { JSON.generate([[[1]]], max_nesting: 2) }
show("max_nesting off") { JSON.generate([[[1]]], max_nesting: 0) }
show("depth") { JSON.generate([1], depth: 2, indent: "  ", array_nl: "\n") }
show("keys") { JSON.generate({1 => 2, :a => 3, nil => 4, 1.5 => 5}) }
show("mixed keys") { $VERBOSE = false; Warning[:deprecated] = true; JSON.generate({:a => 1, "a" => 2}) }
Warning[:deprecated] = false
show("mixed keys refused") { JSON.generate({:a => 1, "a" => 2}, allow_duplicate_key: false) }
show("mixed keys allowed") { JSON.generate({:a => 1, "a" => 2}, allow_duplicate_key: true) }
show("to_s fallback") { o = Object.new; def o.to_s = "OBJ"; JSON.generate([o]) }
show("to_json") { o = Object.new; def o.to_json(*a) = '{"custom":true}'; JSON.generate({"x" => o}) }
show("to_json state") do
  o = Object.new
  def o.to_json(state = nil, *) = "[#{state.class},#{state.depth},#{state.indent.inspect}]"
  JSON.generate([[o]], indent: "  ")
end
show("to_json non-string") { o = Object.new; def o.to_json(*) = 1; JSON.generate([o]) }
show("strict") { JSON.generate([Object.new.class], strict: true) }
show("strict symbol") { JSON.generate([:a, {b: 1}], strict: true) }
show("strict key") { JSON.generate({1 => 2}, strict: true) }
show("strict as_json") { JSON.generate([1..2, {1 => Object}], strict: true, as_json: ->(o, key) { key ? o.to_s : o.to_s.upcase }) }
show("strict nan as_json") { JSON.generate([Float::NAN], strict: true, as_json: ->(o, _) { "nan" }) }
show("Fragment") { JSON.generate({"a" => JSON::Fragment.new("[1, 2]")}) }
show("to IO") { io = StringIO.new; r = JSON.generate([1], nil); JSON.dump([1, "x"], io); [r, io.string] }
show("dump") { JSON.dump({"a" => [1]}) }
show("dump limit") { JSON.dump([[[1]]], 2) }
show("to_json methods") { [[1, {"a" => 2}].to_json, 1.5.to_json, "x".to_json, nil.to_json, true.to_json, false.to_json, 12.to_json, (2**80).to_json, :s.to_json, Object.new.class.to_json] }
show("to_json unfrozen") { [true.to_json.frozen?, nil.to_json.frozen?, JSON.generate(1).frozen?] }
show("to_json arity") { nil.to_json(1, 2) }
show("hash to_json state") { {"a" => [1]}.to_json(JSON::State.new(indent: "  ", object_nl: "\n", array_nl: "\n")) }
show("generate encoding") { JSON.generate(["é"]).encoding }

puts "== generate, plain data"
# What `__json_generate__` writes (#742): the shapes the issue measured, one
# example each, under every layout and escaping option, and documents where
# it gives up partway and the Ruby generator writes the rest.
record = ->(i) do
  {"id" => i, "name" => "user #{i}", "email" => "user#{i}@example.com", "active" => i.even?,
   "score" => i * 1.25 + 0.1, "manager" => nil, "tags" => ["alpha", "beta", "gamma #{i % 7}"],
   "address" => {"street" => "#{i} Main St", "city" => "Springfield", "zip" => format("%05d", i)},
   "bio" => "Line one\nLine \"two\" with é and a tab\t."}
end
plain = {
  "integers" => (-3..3).to_a + [2**62, 2**70, -(2**64)],
  "strings" => ["", "s", "é日本😀", "a\"b\\c\n\u0001", "</script>\u2028\u2029"],
  "floats" => [0.0, -0.0, 1.5, 1.25 * 7 + 0.1, 1e20, 1.5e-7, 1.0e300, 5e-324],
  "hashes" => [{"a" => 1, "b" => 2, "c" => 3}, {a: [], b: {}, c: nil}, {}],
  "records" => (0...3).map(&record),
}
layouts = {
  "default" => {}, "ascii_only" => {ascii_only: true}, "script_safe" => {script_safe: true},
  "layout" => {indent: "\t", space: " ", space_before: " ", object_nl: "\n", array_nl: "\n"},
  "depth" => {depth: 1, indent: "  ", object_nl: "\n", array_nl: "\n"}, "strict" => {strict: true},
}
plain.each do |name, value|
  layouts.each { |layout, opts| show("#{name} #{layout}") { JSON.generate(value, opts) } }
  show("#{name} pretty") { JSON.pretty_generate(value) }
  show("#{name} to_json") { value.to_json }
end
show("max_nesting edge") { [JSON.generate([[{"a" => [1]}]], max_nesting: 4), JSON.generate([[{"a" => [1]}]], max_nesting: 3)] }
show("deep nesting off") { JSON.generate((1..300).reduce([1]) { |acc, _| [acc] }, max_nesting: 0).bytesize }
show("give up deep") { o = Object.new; def o.to_json(*) = "!"; JSON.generate([{"a" => [1, {"b" => [o, 2.5]}]}, [3]], indent: " ", array_nl: "\n") }
show("give up symbol") { JSON.generate([{"a" => [1, :sym]}, {"b" => 2}]) }
show("give up mixed keys") { JSON.generate([{"x" => {:a => 1, "b" => 2}}, {"y" => 3}], allow_duplicate_key: true) }
show("give up subclass") { JSON.generate([{"a" => Class.new(Hash)[1, 2]}, Class.new(Array)[3], Class.new(String).new("s")]) }
show("give up string key subclass") { JSON.generate({Class.new(String).new("k") => 1, "j" => 2}) }
show("give up non-finite") { JSON.generate([[1.5, Float::INFINITY]], allow_nan: true) }
show("give up encoding") { JSON.generate([["ok", "caf\xE9".force_encoding("ISO-8859-1")]]) }
show("give up non-ascii symbol key") { JSON.generate({"é".to_sym => 1}) }
show("singleton hash") { h = {"a" => 1}; def h.each_pair = raise("each_pair"); JSON.generate([h, {"b" => h}]) }
# The C generator walks plain data without sending a message, so none of
# these is called; the issue's measured shapes, one each.
redefined = [[Hash, :each_pair], [Integer, :to_s]]
{
  "5,000 Integers" => (0...5000).to_a,
  "5,000 Strings" => (0...5000).map { |i| "s#{i}" },
  "5,000 Floats" => (0...5000).map { |i| i * 1.5 + 0.1 },
  "500 three-key Hashes" => (0...500).map { |i| {"a" => i, "b" => -i, "c" => [i]} },
  "500 records" => (0...500).map(&record),
}.each do |name, value|
  show("#{name}, nothing called") do
    redefined.each do |klass, method|
      klass.alias_method(:"__fixture_#{method}__", method)
      klass.define_method(method) { |*| raise "#{klass}##{method} called" }
    end
    begin
      generated = [JSON.generate(value), JSON.pretty_generate(value)]
    ensure
      redefined.each do |klass, method|
        klass.alias_method(method, :"__fixture_#{method}__")
        klass.remove_method(:"__fixture_#{method}__")
      end
    end
    generated.map { |json| [json.bytesize, JSON.parse(json) == value, json[0, 48], json[-48..]] }
  end
end

puts "== state"
show("defaults") { JSON::State.new.to_h }
show("configured") { JSON::State.new(indent: "  ", max_nesting: 5, allow_nan: true, script_safe: true, strict: true, depth: 1).to_h }
show("accessors") do
  s = JSON::State.new
  s.indent = "\t"
  s.space = " "
  s.max_nesting = false
  s.buffer_initial_length = 0
  [s.indent, s.space, s.space_before, s.object_nl, s.max_nesting, s.check_circular?, s.buffer_initial_length, s.allow_nan?, s.ascii_only?, s.script_safe?, s.strict?, s.depth, s.as_json]
end
show("bad indent") { JSON::State.new.indent = 1 }
show("bad buffer") { JSON::State.new.buffer_initial_length = "x" }
show("frozen state") { JSON::State.new.freeze.indent = "x" }
show("frozen generate") { JSON::State.new(indent: " ", array_nl: "\n").freeze.generate([1]) }
show("from_state") { [JSON::State.from_state(nil).class, JSON::State.from_state({indent: "x"}).indent, JSON::State.from_state(3).indent] }
show("merge") { JSON::State.new.merge(space: " ").space }
show("instance_variables") { JSON::State.new(indent: "x").instance_variables }
show("generate bad opts") { JSON.generate([1], 3) }
show("state generate io") { io = StringIO.new; JSON::State.new.generate({"a" => 1}, io); io.string }
show("dup") { s = JSON::State.new(indent: "x"); d = s.dup; d.indent = "y"; [s.indent, d.indent] }

puts "== additions"
require "json/add/core"
show("core round trip") do
  values = [1..3, 1...3, Rational(1, 3), Complex(1, 2), /ab+c/ix, :sym, Struct.new("JsonPoint", :x, :y).new(1, 2)]
  values.map { |v| JSON.load(JSON.dump(v)) == v }
end
show("exception") { e = JSON.parse(JSON.dump(RuntimeError.new("boom")), create_additions: true); [e.class, e.message] }
show("create_additions false") { JSON.parse(JSON.dump(1..2)) }
show("unsafe_load") { JSON.unsafe_load(JSON.dump(1..2)) }
show("load deprecation") do
  Warning[:deprecated] = true
  JSON.load(JSON.dump(1..2))
ensure
  Warning[:deprecated] = false
end
show("load nil") { JSON.load(nil) }
show("load proc") { seen = []; JSON.load("[1,[2]]", ->(v) { seen << v }); seen }
show("Coder") { c = JSON::Coder.new { |x| x.to_s }; [c.dump([Object, 1]), c.load("[1, 2.5]")] }
