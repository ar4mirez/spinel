# Each thread runs on an OS thread of its own (#328).
sleeper = Thread.new { sleep }
Thread.pass until sleeper.stop?
puts sleeper.native_thread_id == Thread.current.native_thread_id ? "shared" : "own"

# Opening a FIFO blocks in the kernel until its other end is opened. It is
# made outside the lock, so the thread that opens the other end runs.
fifo = File.join(ARGV[0], "fifo")
File.mkfifo(fifo)
reader = Thread.new { File.open(fifo, "r") { |f| f.read } }
ticks = 0
ticker = Thread.new { 3.times { sleep 0.01; ticks += 1 } }
ticker.join
File.open(fifo, "w") { |f| f.write("through") }
puts reader.value
puts ticks == 3 ? "ran meanwhile" : "stopped: #{ticks}"

# The program's end kills the threads still alive, after `at_exit`, running
# the `ensure` clauses they are in.
at_exit { puts sleeper.alive? ? "alive at exit" : "dead at exit" }
last = Thread.new do
  sleep
ensure
  puts "killed at the end"
end
Thread.pass until last.stop?
