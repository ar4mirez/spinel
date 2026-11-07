# A signal sent to a child the moment `fork` returns is the child's to
# handle, with the handler it inherited (#44).
parent = Process.pid
trap("TERM") { puts(Process.pid == parent ? "parent" : "child"); $stdout.flush; exit!(0) }
pid = fork do
  sleep 5
  puts "lost"
  exit!(1)
end
Process.kill(:TERM, pid)
Process.wait(pid)
puts $?.exitstatus
