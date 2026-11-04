# `String#index` and `String#byteindex` with a String whose match is at the
# start, the calls #617 was filed for: each should cost about the same
# whatever the length of the String it searches.
#
#   scripts/bench.sh string_index
#   target/release/spinel run bench/string_index.rb
#   ruby --yjit bench/string_index.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

calls = 1000
[1_000, 10_000, 100_000].each do |n|
  s = "x" * n + "\n"
  time("index(\"x\", 0)      n=#{n}") { calls.times { s.index("x", 0) } }
  time("byteindex(\"x\", 0)  n=#{n}") { calls.times { s.byteindex("x", 0) } }
end
