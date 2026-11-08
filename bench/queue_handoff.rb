# Handing items through a Queue and a SizedQueue: push and pop in one thread,
# with and without a timeout, a producer and a consumer taking turns, and a
# Monitor entered and left (#46).
#
#   target/release/spinel run bench/queue_handoff.rb
#   ruby --yjit bench/queue_handoff.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "monitor"

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
# The threaded cases stay small while `Array#shift` is linear (#733).
THREADED_N = 1_000

time("push, pop         ") { q = Queue.new; i = 0; while i < N; q << i; q.pop; i += 1; end }
time("push, pop timeout:") { q = Queue.new; i = 0; while i < N; q << i; q.pop(timeout: 1); i += 1; end }
time("producer, consumer") do
  q = Queue.new
  t = Thread.new { i = 0; while i < THREADED_N; q.pop; i += 1; end }
  i = 0
  while i < THREADED_N; q << i; i += 1; end
  t.join
end
time("SizedQueue(16)    ") do
  q = SizedQueue.new(16)
  t = Thread.new { i = 0; while i < THREADED_N; q.pop; i += 1; end }
  i = 0
  while i < THREADED_N; q << i; i += 1; end
  t.join
end
time("Monitor enter/exit") { m = Monitor.new; i = 0; while i < N; m.synchronize { m.synchronize {} }; i += 1; end }
