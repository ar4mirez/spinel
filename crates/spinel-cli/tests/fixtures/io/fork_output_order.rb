# Run with stdout and stderr both going to one file. `$stdout` is not a
# terminal there, so it buffers; `$stderr` is `sync` and writes at once.
# `fork`, `system` and `spawn` flush `$stdout` and `$stderr` first, a child's
# buffer is flushed when it exits, and the parent's last line waits for the
# program's end.
puts "parent: before fork"
$stderr.puts "parent: stderr before fork"
pid = fork do
  puts "child: stdout"
  $stderr.puts "child: stderr"
end
Process.wait(pid)
puts "parent: after fork"
system("echo", "system: child")
print "parent: before spawn\n"
Process.wait(spawn("echo", "spawn: child"))
$stderr.puts "parent: stderr at end"
puts "parent: last line"
