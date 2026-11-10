# `String#tr` and its family over a byte-wide table, and `Random#bytes`: the
# calls #744 was filed for, which cost tens of microseconds a byte while a
# character walk did them. Each should cost a few nanoseconds a byte, and grow
# linearly with the String.
#
#   scripts/bench.sh string_tr
#   target/release/spinel run bench/string_tr.rb
#   ruby --yjit bench/string_tr.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label, n)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  n.times { yield }
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1_000_000 / n
  puts "#{label}: #{elapsed.round(1)} us"
end

every = (0..255).map(&:chr).join.b
table = ("abcdefgh" * 33).byteslice(0, 256)
# `N=1` for a build that takes seconds a call, as the one before #744 did.
n = Integer(ENV.fetch("N", "200"))
[2_000, 4_000, 8_000].each do |size|
  b = ("a".."z").to_a.join.b * (size / 26)
  time("tr over a 256-entry table  #{size} bytes", n) { b.tr(every, table) }
  time("tr(\"a-z\", \"A-Z\")           #{size} bytes", n) { b.tr("a-z", "A-Z") }
  time("Random#bytes               #{size} bytes", n) { Random.new(3).bytes(size) }
  time("tr_s(\"a-m\", \"x\")           #{size} bytes", n) { b.tr_s("a-m", "x") }
  time("delete(\"a-m\")              #{size} bytes", n) { b.delete("a-m") }
  time("squeeze(\"a-z\")             #{size} bytes", n) { b.squeeze("a-z") }
  time("count(\"a-z\", \"^m\")         #{size} bytes", n) { b.count("a-z", "^m") }
end
