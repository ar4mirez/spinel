# The built-in json (#55): parsing and generating a medium document, about
# 130 KB of the shape an API answers with — an Array of records, each a Hash
# of Strings, Integers, Floats, booleans, nil and a nested Array and Hash.
#
#   target/release/spinel run bench/json.rb
#   ruby --yjit bench/json.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "json"

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(2)} ms"
end

records = (0...500).map do |i|
  {
    "id" => i,
    "name" => "user #{i}",
    "email" => "user#{i}@example.com",
    "active" => i.even?,
    "score" => i * 1.25 + 0.1,
    "manager" => nil,
    "tags" => ["alpha", "beta", "gamma #{i % 7}"],
    "address" => { "street" => "#{i} Main St", "city" => "Springfield", "zip" => format("%05d", i) },
    "bio" => "Line one\nLine \"two\" with é and a tab\t.",
  }
end
document = JSON.generate(records)
puts "document: #{document.bytesize} bytes"

time("JSON.parse          ") { JSON.parse(document) }
time("JSON.parse symbols  ") { JSON.parse(document, symbolize_names: true) }
time("JSON.generate       ") { JSON.generate(records) }
time("JSON.pretty_generate") { JSON.pretty_generate(records) }
time("to_json             ") { records.to_json }
