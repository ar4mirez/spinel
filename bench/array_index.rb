# The Integer paths of `Array#[]`, `#[]=`, `#at`, `#pop` and `#+`, each
# 200,000 times. Those primitives hand any other index to Ruby (#21); this is
# what shows the Integer path did not pay for it.
#
#   target/release/spinel run bench/array_index.rb
#   ruby --yjit bench/array_index.rb
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

N = 200_000

a = Array.new(N, 1)
b = [1, 2]
r = 1..2

time("a[i]        ") { i = 0; while i < N; a[i]; i += 1; end }
time("a[-i]       ") { i = 1; while i < N; a[-i]; i += 1; end }
time("a[i] = x    ") { i = 0; while i < N; a[i] = i; i += 1; end }
time("a.at(i)     ") { i = 0; while i < N; a.at(i); i += 1; end }
time("a[i, 2]     ") { i = 0; while i < N; a[i, 2]; i += 1; end }
time("a[1..2]     ") { i = 0; while i < N; a[r]; i += 1; end }
time("push; pop(1)") { i = 0; while i < N; a.push(i); a.pop(1); i += 1; end }
time("b + b       ") { i = 0; while i < N; b + b; i += 1; end }
