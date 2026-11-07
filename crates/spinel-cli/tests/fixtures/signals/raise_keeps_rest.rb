# SIGUSR1 and SIGINT both arrive while a `system` waits for its child. The
# one handled first may raise, an `Interrupt` the program rescues; the other
# still waits, and runs at the next safe point (#44).
seen = []
trap("USR1") { seen << "usr1" }
pid = Process.pid
begin
  system("sh", "-c", "kill -USR1 #{pid}; kill -INT #{pid}; sleep 0.2")
  sleep 0.5
rescue Interrupt
  seen << "interrupt"
end
sleep 0.1
puts seen.sort.inspect
