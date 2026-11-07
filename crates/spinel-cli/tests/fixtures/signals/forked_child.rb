# A child after `fork` inherits the parent's handlers and wakes on a pipe of
# its own: a signal ends its sleep's wait while the parent waits too (#44).
trap("USR1") { puts "parent" }
child = fork do
  trap("USR1") { puts "child"; $stdout.flush; exit!(0) }
  spawn("sh", "-c", "sleep 0.1; kill -USR1 #{Process.pid}")
  sleep 5
  puts "not woken"
  exit!(1)
end
sleep 0.05 until Process.wait(child, Process::WNOHANG)
puts $?.exitstatus
