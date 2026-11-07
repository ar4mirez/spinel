# SIGUSR1 and SIGUSR2 both wait when a `system` returns. USR1's handler
# runs first and sleeps; USR2's waits for it to return rather than running
# inside it, as CRuby's trap mask keeps it (#44).
seen = []
trap("USR1") { seen << "a1"; sleep 0.05; seen << "a2" }
trap("USR2") { seen << "b" }
pid = Process.pid
system("sh", "-c", "kill -USR1 #{pid}; kill -USR2 #{pid}; sleep 0.2")
sleep 0.2
p seen
