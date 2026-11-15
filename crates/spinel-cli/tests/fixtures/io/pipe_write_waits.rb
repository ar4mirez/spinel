# A write bigger than a pipe holds, while another thread reads the other end:
# the write takes what fits and waits for the rest, so the reader runs. Then
# the same through the write buffer, whose flush waits too, and `syswrite`,
# which takes what fits of a full pipe once there is room.
r, w = IO.pipe
reader = Thread.new { r.read.bytesize }
w.write("x" * 1_000_000)
w.close
p reader.value

r, w = IO.pipe
w.sync = false
reader = Thread.new { r.read.bytesize }
w.write("x" * 1_000_000)
w.write("y" * 100)
w.flush
w.write("z" * 10)
w.close
p reader.value

r, w = IO.pipe
p w.syswrite("x" * 1_000_000)
reader = Thread.new { sleep 0.2; r.read(16384).bytesize }
p w.syswrite("y" * 10)
p reader.value
