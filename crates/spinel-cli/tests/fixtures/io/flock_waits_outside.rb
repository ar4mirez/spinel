# A thread waits for a lock another File holds, outside the per-Ractor lock
# (#41): the other threads go on meanwhile, it sleeps, and a kill ends its
# wait. ARGV[0] is a file the test made. R2 of the plan, waiting for the
# ticker rather than a fixed time.
path = ARGV[0]
ticks = 0
f1 = File.open(path, "w")
f1.flock(File::LOCK_EX)
t = Thread.new { File.open(path, "w") { |f2| f2.flock(File::LOCK_EX) } }
ticker = Thread.new { 10.times { sleep 0.01; ticks += 1 } }
ticker.join(30)
Thread.pass while t.status && t.status != "sleep"
p [:ticks, ticks, t.status]
t.kill
p t.join(30) ? :joined : :still_blocked
