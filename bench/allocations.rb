# Allocation-heavy loops for #690: `objspace`'s allocation tracing hooks the
# allocator and the interpreter loop, and with tracing off that hook has to
# cost nothing a benchmark can see. The last rows turn tracing on, so the
# price of having it on is measured beside the price of having it off.
#
#   target/release/spinel run bench/allocations.rb
#   ruby --yjit bench/allocations.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

require 'objspace'

def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  end.min
  puts "#{label}: #{best.round(1)} ms"
end

N = 200_000

def allocate
  i = 0
  while i < N
    Object.new
    [i, i]
    "s#{i}"
    i += 1
  end
end

time("objects, arrays, strings  n=#{N}") { allocate }
time("map to strings            n=#{N}") { (1..N).map(&:to_s) }
ObjectSpace.trace_object_allocations do
  time("traced: objects, arrays, strings") { allocate }
end
ObjectSpace.trace_object_allocations_clear
