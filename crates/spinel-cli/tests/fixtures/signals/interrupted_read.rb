# SIGINT reaches a main thread blocked reading a pipe that nothing writes, and
# its default handler raises Interrupt there (#44), as Ctrl-C at `gets` does.
reader, writer = IO.pipe
pid = fork do
  reader.close
  sleep 0.2
  Process.kill(:INT, Process.ppid)
end
begin
  reader.gets
  puts "read"
rescue Interrupt => e
  p e
end
Process.wait(pid)
writer.close
