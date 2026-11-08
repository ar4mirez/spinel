# `Queue#pop`, `SizedQueue#push` and `Monitor#enter` block without spinning
# (#46): each waits 0.3 s for another thread, and the process spends a small
# part of that on the CPU. A waiter that spun on `Thread.pass` would spend
# all of it.
require "monitor"

def cpu = Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID)
def clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def idle(name)
  wall = clock
  used = cpu
  yield
  wall = clock - wall
  used = cpu - used
  puts(wall >= 0.25 && used < 0.1 ? "#{name} slept" : "#{name}: #{used} s of CPU in #{wall} s")
end

q = Queue.new
Thread.new { sleep 0.3; q << :item }
idle("pop") { q.pop }

sq = SizedQueue.new(1)
sq << :full
Thread.new { sleep 0.3; sq.pop }
idle("push") { sq.push(:more) }

m = Monitor.new
held = Queue.new
Thread.new { m.synchronize { held << true; sleep 0.3 } }
held.pop
idle("enter") { m.synchronize {} }
