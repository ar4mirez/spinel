# SIGTERM arrives while the program's last call, a `system`, waits for its
# child: the program still ends by it, at its last safe point (#44).
pid = Process.pid
system("sh", "-c", "kill -TERM #{pid}; sleep 0.2")
puts "after"
