# A thread a trap handler starts while the main thread waits for a child
# runs before the child ends: here it ends the child (#44).
pid = fork { sleep 5; exit!(0) }
trap("USR1") { Thread.new { Process.kill(:TERM, pid) rescue nil } }
spawn("sh", "-c", "sleep 0.2; kill -USR1 #{Process.pid}")
Process.wait(pid)
p $?.termsig
Process.wait
