# The loops #299 was filed for: substitution, splitting, scanning and indexing
# over a String, and sorting an Array — each at three sizes, so a loop that is
# quadratic in its subject shows as one.
#
#   target/release/spinel run bench/string_loops.rb
#   ruby --yjit bench/string_loops.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

[500, 2000, 8000].each do |n|
  s = "ab " * (n / 3)
  time("gsub regexp, block  n=#{n}") { s.gsub(/ /) { "-" } }
  time("gsub string         n=#{n}") { s.gsub(" ", "-") }
  time("split regexp        n=#{n}") { s.split(/ /) }
  time("split string        n=#{n}") { s.split(" ") }
  time("scan                n=#{n}") { s.scan(/\w+/) }
  time("each_char           n=#{n}") { s.each_char { |c| c } }
  time("index by character  n=#{n}") { i = 0; i += 1 while i < s.size && s[i] }
end

[1000, 4000, 16000].each do |n|
  strings = (1..n).map { |i| "s#{(i * 7919) % n}" }
  integers = (1..n).map { |i| (i * 7919) % n }
  time("sort strings        n=#{n}") { strings.sort }
  time("sort integers       n=#{n}") { integers.sort }
  time("sort_by             n=#{n}") { strings.sort_by { |x| x } }
  time("sort with a block   n=#{n}") { integers.sort { |x, y| y <=> x } }
end
