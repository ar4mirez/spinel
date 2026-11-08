# Draining an Array from the front: filling one and shifting it empty, a
# `while (x = a.shift)` loop, a work list that pushes at the back as it shifts
# from the front, `shift(n)`, and `unshift`, each over 20,000 elements (#733).
# Before #733 every shift and unshift copied the rest of the array.
#
#   target/release/spinel run bench/array_shift.rb
#   ruby --yjit bench/array_shift.rb
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

N = 20_000

time("push, then shift  ") { a = []; i = 0; while i < N; a << i; i += 1; end; i = 0; while i < N; a.shift; i += 1; end }
time("while (x = shift) ") { a = Array.new(N, 1); while (x = a.shift); end }
time("work list         ") do
  work = [0]
  while (node = work.shift)
    child = node * 2 + 1
    work << child if child < N
    work << child + 1 if child + 1 < N
  end
end
time("shift(2)          ") { a = Array.new(N, 1); while a.shift(2).size > 0; end }
time("unshift           ") { a = []; i = 0; while i < N; a.unshift(i); i += 1; end }
