# Filling a Hash, and reading it back: String and Integer keys, a table
# sizes from small to past the point a linear one gives out at (#729), and
# the small hashes keyword arguments and literals make, which never grow a
# table at all.
#
#   target/release/spinel run bench/hash_insert.rb
#   ruby --yjit bench/hash_insert.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(1)} ms"
end

N = 10_000

strings = (0...N).map { |i| "k#{i}" }

time("h[str] = v, 2000   ") { h = {}; i = 0; while i < 2000; h[strings[i]] = i; i += 1; end }
time("h[str] = v, 10000  ") { h = {}; i = 0; while i < N; h[strings[i]] = i; i += 1; end }
time("h[int] = v, 10000  ") { h = {}; i = 0; while i < N; h[i] = i; i += 1; end }

full = {}
i = 0
while i < N
  full[i] = i
  i += 1
end
time("h[int], 10000      ") { i = 0; while i < N; full[i]; i += 1; end }
time("{a:, b:}[:c] = v   ") { i = 0; while i < N; x = {a: 1, b: 2}; x[:c] = 3; i += 1; end }
