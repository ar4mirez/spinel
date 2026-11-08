# The vendored optparse (#51), run over switches, arguments, conversions,
# completion, help and errors. Expected output is ruby 4.0.9's.
require "optparse"
require "optparse/time"
require "optparse/uri"
require "optparse/kwargs"

def show(label)
  print label, ": "
  p yield
rescue OptionParser::ParseError => e
  puts "#{e.class}: #{e.message}"
end

def parser(opts)
  OptionParser.new do |o|
    o.banner = "Usage: tool [options]"
    o.program_name = "tool"
    o.version = "1.2.3"
    o.separator ""
    o.separator "Specific options:"
    o.on("-v", "--[no-]verbose", "Run verbosely") { |v| opts[:verbose] = v }
    o.on("-n", "--name NAME", String, "A name") { |v| opts[:name] = v }
    o.on("-c", "--count N", Integer, "A count") { |v| opts[:count] = v }
    o.on("-r", "--ratio R", Float, "A ratio") { |v| opts[:ratio] = v }
    o.on("--num X", Numeric) { |v| opts[:num] = v }
    o.on("--oct X", OptionParser::OctalInteger) { |v| opts[:oct] = v }
    o.on("--dec X", OptionParser::DecimalInteger) { |v| opts[:dec] = v }
    o.on("-l", "--list A,B", Array, "A list") { |v| opts[:list] = v }
    o.on("-t", "--type TYPE", [:text, :binary, :auto], "Select type") { |v| opts[:type] = v }
    o.on("--code CODE", {"one" => 1, "two" => 2}) { |v| opts[:code] = v }
    o.on("-o", "--optional [ARG]", "Optional") { |v| opts[:optional] = v }
    o.on("--flag=BOOL", TrueClass) { |v| opts[:flag] = v }
    o.on("--re PAT", /\A(\d+)-(\d+)\z/) { |all, a, b| opts[:re] = [all, a, b] }
    o.on("--time T", Time) { |v| opts[:time] = v.utc.year }
    o.on("--uri U", URI) { |v| opts[:uri] = v.host }
    o.on("-x") { opts[:x] = true }
    o.on("-y") { opts[:y] = true }
    o.separator ""
    o.on_tail("-h", "--help", "Show help") { opts[:help] = true }
  end
end

def run(argv)
  opts = {}
  rest = parser(opts).parse(argv)
  [opts, rest]
end

show("basic") { run(%w[-v --name bob -c 3 file1 file2]) }
show("no-verbose") { run(%w[--no-verbose]) }
show("equals") { run(%w[--name=alice --count=-7]) }
show("attached") { run(%w[-nbob -c42]) }
show("clustered") { run(%w[-xyv]) }
show("abbrev") { run(%w[--verb --na zed]) }
show("float") { run(%w[-r 2.5e1]) }
show("numeric") { run(%w[--num 3/4]) }
show("octal") { run(%w[--oct 017 --dec 010]) }
show("bad octal") { run(%w[--oct 0x1f]) }
show("list") { run(%w[-l a,b,c]) }
show("enum") { run(%w[-t bin]) }
show("hash") { run(%w[--code tw]) }
show("optional none") { run(%w[-o -v]) }
show("optional given") { run(%w[-o val]) }
show("bool") { run(%w[--flag=no]) }
show("regexp") { run(%w[--re 12-34]) }
show("time") { run(["--time", "2024-02-29 13:05:09 UTC"]) }
show("uri") { run(%w[--uri http://example.com/x]) }
show("terminator") { run(%w[-v -- -x --name]) }
show("permute") { run(%w[a -v b -c 1 c]) }
show("order") { opts = {}; [parser(opts).order(%w[a -v b]), opts] }
show("order!") { a = %w[-v x -y]; opts = {}; parser(opts).order!(a); [a, opts] }
show("parse!") { a = %w[-v x -y]; opts = {}; parser(opts).parse!(a); [a, opts] }
show("into") do
  h = {}
  OptionParser.new { |o| o.on("-a"); o.on("-b VAL", Integer); o.on("--long-name [X]") }.parse(%w[-a -b 5 --long-name], into: h)
  h
end
show("missing") { run(%w[--name]) }
show("invalid") { run(%w[--bogus]) }
show("invalid short") { run(%w[-q]) }
show("bad int") { run(%w[-c abc]) }
show("bad float") { run(%w[-r x]) }
show("bad enum") { run(%w[-t zip]) }
show("ambiguous") { run(%w[--n x]) }
show("needless") { run(%w[--verbose=yes]) }
show("bad regexp") { run(%w[--re 12]) }
show("require_exact") do
  o = parser({})
  o.require_exact = true
  o.parse(%w[--verb])
end
show("raise_unknown off") do
  o = OptionParser.new { |x| x.on("-a") }
  o.raise_unknown = false
  o.parse(%w[-a --zzz b])
end
show("environment") do
  ENV["TOOL"] = "-v --name envy"
  opts = {}
  o = parser(opts)
  o.environment("TOOL")
  opts
end
show("getopts") { OptionParser.new.getopts(%w[-a -b 1 --foo --bar=2 rest], "ab:", "foo", "bar:", "baz:9") }
show("summary width") { parser({}).summary_width }

puts parser({}).help
puts parser({}).ver
puts parser({}).to_a.size

begin
  raise OptionParser::InvalidOption.new("--zap")
rescue OptionParser::ParseError => e
  e.set_option("--zap", true)
  p [e.reason, e.args, e.message]
end

custom = OptionParser.new
custom.accept(Range, /\A(\d+)\.\.(\d+)\z/) { |s, a, b| (a.to_i..b.to_i) }
r = nil
custom.on("--range R", Range) { |v| r = v }
custom.parse(%w[--range 2..5])
p r

def kw(alpha: 1, beta_gamma: nil); end
k = OptionParser.new
kh = {}
k.define_by_keywords(kh, method(:kw), alpha: Integer, beta_gamma: String)
k.parse(%w[--alpha 3 --beta-gamma x])
p kh
