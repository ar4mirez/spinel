# A pipe that is blocking — as one a program inherits is — with threads
# (#41): a megabyte written to it while another thread reads it, and a
# thread writing to or reading from it cut short by a kill or a `close` in
# another thread. Its write and read are made outside the per-Ractor lock.
require "io/nonblock"

def waiting(thread)
  Thread.pass while thread.status && thread.status != "sleep"
  sleep 0.05
  Thread.pass while thread.status && thread.status != "sleep"
end

r, w = IO.pipe
w.nonblock = false
reader = Thread.new { r.read.bytesize }
w.write("x" * 1_000_000)
w.close
p reader.value

r, w = IO.pipe
w.nonblock = false
w.sync = true
ticks = 0
ticker = Thread.new { 10.times { sleep 0.01; ticks += 1 } }
t = Thread.new { w.write("x" * 1_000_000) }
ticker.join(30)
waiting(t)
p [:ticks, ticks, t.status]
t.kill
p t.join(30) ? :joined : :still_blocked

r, w = IO.pipe
w.nonblock = false
w.sync = true
t = Thread.new do
  w.write("x" * 1_000_000)
  :written
rescue IOError => e
  [e.class, e.message]
end
waiting(t)
w.close
p t.value

r, w = IO.pipe
r.nonblock = false
t = Thread.new do
  r.read(10)
rescue IOError => e
  [e.class, e.message]
end
waiting(t)
r.close
p t.value
