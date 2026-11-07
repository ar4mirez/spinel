# `system` inside a trap handler, with another signal waiting, waits for its
# child without spinning, and the waiting signal is handled once the handler
# returns (#44).
trap("USR2") { puts "usr2" }
trap("USR1") do
  Process.kill(:USR2, Process.pid)
  cpu = Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID)
  system("sleep", "0.5")
  p Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID) - cpu < 0.25
end
Process.kill(:USR1, Process.pid)
sleep 0.2
puts "end"
