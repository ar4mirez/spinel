# Waiting for a live process that is not a child raises ECHILD at once, as
# it did before the pidfd wait (#44).
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
begin
  Process.wait(Process.ppid)
  puts "waited"
rescue Errno::ECHILD
  puts "ECHILD"
end
p Process.clock_gettime(Process::CLOCK_MONOTONIC) - started < 1
