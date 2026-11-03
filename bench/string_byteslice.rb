# `String#byteslice` with Integers against `String#getbyte`, the calls #588
# was filed for: a short slice should cost about what reading one byte does,
# whatever the length of the String it is taken from.
#
#   scripts/bench.sh string_byteslice
#   target/release/spinel run bench/string_byteslice.rb
#   ruby --yjit bench/string_byteslice.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

s = "x" * 50_000
s2 = "x" * 10
n = 50_000
time("getbyte(0)              n=#{n}") { n.times { s.getbyte(0) } }
time("byteslice(1, 9)  of 10  n=#{n}") { n.times { s2.byteslice(1, 9) } }
time("byteslice(1, 9)  of 50K n=#{n}") { n.times { s.byteslice(1, 9) } }
time("byteslice(3)     of 10  n=#{n}") { n.times { s2.byteslice(3) } }
time("byteslice(1, 49_999)    n=#{n}") { n.times { s.byteslice(1, 49_999) } }
