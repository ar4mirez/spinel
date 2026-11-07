# A trap's handler runs on the main thread while it sleeps, and the sleep goes
# on for the rest of its time: a signal does not cut it short (#44).
ran = []
Signal.trap(:USR1) { ran << Thread.current.equal?(Thread.main) }
Thread.new { sleep 0.1; Process.kill(:USR1, Process.pid) }
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
sleep 0.5
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
p ran
puts elapsed >= 0.45 ? "slept on" : "cut short after #{elapsed}"
