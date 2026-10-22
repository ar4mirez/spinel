# Equality on numbers, reached each way a program reaches it: the `==`
# instruction, a send of `==` or `===`, and the core methods that ask `===`
# of an element (#345, #349). Each 200,000 times.
#
#   target/release/spinel run bench/numeric_eq.rb
#   ruby --yjit bench/numeric_eq.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

N = 200_000

a = Array.new(N) { |i| i % 7 }
f = 1.5
time("i == 3               ") { i = 0; while i < N; i == 3; i += 1; end }
time("i != 3               ") { i = 0; while i < N; i != 3; i += 1; end }
time("f == 1.5             ") { i = 0; while i < N; f == 1.5; i += 1; end }
time("i.send(:==, 3)       ") { i = 0; while i < N; i.send(:==, 3); i += 1; end }
time("3 === i              ") { i = 0; while i < N; 3 === i; i += 1; end }
time("1.5 === f            ") { i = 0; while i < N; 1.5 === f; i += 1; end }
time("Array#grep(3)        ") { a.grep(3) }
time("Array#count(3)       ") { a.count(3) }
