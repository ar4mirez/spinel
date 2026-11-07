# A signal that arrives while the main thread waits for a child, in `system`
# or `Process.wait`, is acted on at once rather than when the child exits: a
# trap runs and the wait goes on, and an untrapped SIGTERM ends the program
# inside the wait (#44). Measured against ruby 4.0.9.
order = []
at_exit { p order }
trap(:USR1) { order << :handler }
spawn("sh", "-c", "sleep 0.2; kill -USR1 #{Process.pid}")
system("sleep", "1")
order << :system
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
at_exit { p(Process.clock_gettime(Process::CLOCK_MONOTONIC) - started < 4) }
pid = spawn("sh", "-c", "sleep 0.2; kill -TERM #{Process.pid}; sleep 5")
Process.wait(pid)
order << :after
