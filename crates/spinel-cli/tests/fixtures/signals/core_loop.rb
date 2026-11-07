# A signal from another process reaches a main thread that spends nearly all
# its time in the core library — `Time.now` and `Time#-` are Ruby — at the
# first instruction of the program's own that comes after it (#44).
handled = false
Signal.trap(:USR1) { handled = true }
pid = fork { sleep 0.2; Process.kill(:USR1, Process.ppid) }
started = Time.now
nil until handled || Time.now - started > 10
Process.wait(pid)
puts handled ? "handled" : "never handled"
