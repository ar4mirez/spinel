# The built-in psych (#108): a config-sized document loaded and dumped, a
# large one parsed to events with a handler that does nothing (libyaml and
# the event replay alone) and to a tree, and that tree emitted again.
#
#   target/release/spinel run bench/psych.rb
#   ruby --yjit bench/psych.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "yaml"

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(2)} ms"
end

config = {
  "name" => "service",
  "version" => 3,
  "enabled" => true,
  "ratio" => 0.75,
  "hosts" => %w[alpha beta gamma],
  "database" => {"adapter" => "postgresql", "pool" => 5, "timeout" => 5000},
  "notes" => "line one\nline two\n",
}
config_yaml = YAML.dump(config)
large = (1..2000).map { |i| {"id" => i, "name" => "item #{i}", "tags" => %w[a b c], "price" => i * 1.5} }
large_yaml = YAML.dump(large)
tree = Psych.parse_stream(large_yaml)

time("1k YAML.load, config      ") { 1000.times { YAML.load(config_yaml) } }
time("1k YAML.dump, config      ") { 1000.times { YAML.dump(config) } }
time("parse 2k records, events  ") { Psych::Parser.new(Psych::Handler.new).parse(large_yaml) }
time("parse 2k records, tree    ") { Psych.parse_stream(large_yaml) }
time("load 2k records           ") { YAML.load(large_yaml) }
time("emit 2k-record tree       ") { tree.yaml }
time("dump 2k records           ") { YAML.dump(large) }
