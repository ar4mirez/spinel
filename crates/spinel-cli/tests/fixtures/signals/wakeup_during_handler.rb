# A wait a signal ends hands back to its caller once the handler has run, to
# look at what it waits for again: the thread it waits on may have ended,
# unlocked or pushed while the handler ran, and so woken it while it was not
# waiting (#735). Each thread here does that only once the handler started,
# and the handler waits for it.
started = done = false
trap(:USR1) do
  started = true
  Thread.pass until done
end
signal = -> { started = done = false; Process.kill(:USR1, Process.pid); Thread.pass until started }

# `join`: the thread ends while the handler runs.
t = Thread.new { signal.call; done = true }
t.join
puts "join"

# `Mutex#lock`: the owner unlocks while the handler runs.
m = Mutex.new
t = Thread.new do
  m.lock
  Thread.pass until Thread.main.stop?
  signal.call
  m.unlock
  done = true
end
Thread.pass until m.locked?
m.lock
puts "lock"
m.unlock
t.join

# `Queue#pop`: the push comes while the handler runs.
q = Queue.new
t = Thread.new do
  Thread.pass until Thread.main.stop?
  signal.call
  q << :pushed
  done = true
end
p q.pop
t.join

# `Mutex#sleep`: a handled signal ends it, as CRuby's does.
m = Mutex.new
t = Thread.new do
  Thread.pass until Thread.main.stop?
  signal.call
  done = true
end
started_at = Process.clock_gettime(Process::CLOCK_MONOTONIC)
m.synchronize { m.sleep(5) }
p Process.clock_gettime(Process::CLOCK_MONOTONIC) - started_at < 4
t.join
