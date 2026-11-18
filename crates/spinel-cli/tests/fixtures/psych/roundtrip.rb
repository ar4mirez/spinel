# The built-in psych (#108) against CRuby's: for each case in cases.rb,
# whether this Ruby's YAML.dump is byte for byte what CRuby's Psych wrote
# (yaml/, from make.rb), and what loading CRuby's YAML gives back. Every
# line is what ruby 4.0.9 prints (roundtrip.out); a line reading MISMATCH
# is wrong on any Ruby, since CRuby wrote the files this compares with.
require_relative "cases"

DIR = File.join(__dir__, "yaml")

# A deterministic account of a loaded object: its class and contents,
# never an address, so two Rubies print the same text for equal objects.
def describe(object, seen = {})
  return "<cycle>" if seen.key?(object.object_id)
  case object
  when String then "#{object.inspect}@#{object.encoding}"
  when Float then object.nan? ? "NaN" : object.inspect
  when nil, true, false, Integer, Symbol, Regexp, Rational, Complex then object.inspect
  when Array
    seen = seen.merge(object.object_id => true)
    "[#{object.map { |e| describe(e, seen) }.join(", ")}]"
  when Hash
    seen = seen.merge(object.object_id => true)
    "{#{object.map { |k, v| "#{describe(k, seen)} => #{describe(v, seen)}" }.join(", ")}}"
  when Time then "Time(#{object.strftime("%Y-%m-%d %H:%M:%S.%N %:z")} utc=#{object.utc?})"
  when DateTime then "DateTime(#{object.iso8601})"
  when Date then "Date(#{object.iso8601})"
  when Range then "Range(#{describe(object.begin)}, #{describe(object.end)}, #{object.exclude_end?})"
  when Struct then "#{object.class}(#{object.to_a.map { |e| describe(e, seen) }.join(", ")})"
  when Exception then "#{object.class}(#{object.message.inspect})"
  else
    ivars = object.instance_variables.map { |iv| "#{iv}=#{describe(object.instance_variable_get(iv), seen)}" }
    "#{object.class}{#{ivars.join(", ")}}"
  end
end

# Where two dumps first differ, for a mismatch to show.
def first_difference(expected, actual)
  e = expected.lines
  a = actual.lines
  index = e.zip(a).index { |x, y| x != y } || [e.size, a.size].min
  "line #{index + 1}: CRuby #{e[index].inspect}, here #{a[index].inspect}"
end

def report(name, dumped, expected, loaded, original)
  dump_ok = dumped.b == expected.b
  load_ok = describe(loaded) == describe(original)
  status = dump_ok && load_ok ? "ok      " : "MISMATCH"
  line = "#{status} #{name} #{describe(loaded)}"
  line << " (dump #{first_difference(expected, dumped)})" unless dump_ok
  line << " (loaded differs from #{describe(original)})" unless load_ok
  puts line
end

puts "Psych #{Psych::VERSION}, libyaml #{Psych::LIBYAML_VERSION}"

PsychCases::DUMPS.each do |name, object, options|
  expected = File.binread(File.join(DIR, "#{name}.yaml"))
  dumped = options ? YAML.dump(object, **options) : YAML.dump(object)
  report(name, dumped, expected, YAML.unsafe_load(expected), object)
  if options.nil? && object.to_yaml != dumped
    puts "MISMATCH #{name} to_yaml differs from YAML.dump"
  end
end

aliased = YAML.unsafe_load(File.binread(File.join(DIR, "aliases.yaml")))
puts "aliases share one object: #{aliased["first"].equal?(aliased["second"]) && aliased["third"][0].equal?(aliased["first"])}"

PsychCases::STREAMS.each do |name, objects|
  expected = File.binread(File.join(DIR, "#{name}.yaml"))
  dumped = YAML.dump_stream(*objects)
  report(name, dumped, expected, YAML.load_stream(expected), objects)
end

PsychCases::REEMIT.each do |name, source|
  expected = File.binread(File.join(DIR, "reemit_#{name}.yaml"))
  tree = Psych.parse_stream(source)
  emitted = tree.yaml
  status = emitted.b == expected.b ? "ok      " : "MISMATCH"
  marks = tree.each.map do |node|
    "#{node.class.name.split("::").last}@#{node.start_line}:#{node.start_column}-#{node.end_line}:#{node.end_column}"
  end
  line = "#{status} reemit_#{name} #{marks.join(" ")}"
  line << " (#{first_difference(expected, emitted)})" unless emitted.b == expected.b
  puts line
end

PsychCases::ERRORS.each do |name, action|
  result =
    begin
      "=> #{describe(action.call)}"
    rescue Exception => e
      "!! #{e.class}: #{e.message}"
    end
  puts "#{name} #{result}"
end
