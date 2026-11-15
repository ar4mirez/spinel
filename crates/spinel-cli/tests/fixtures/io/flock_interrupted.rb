# A thread waiting for a lock outside the per-Ractor lock (#41) is cut short
# by another thread's raise, by the File it waits with closed in another
# thread, and by the program's end, whose kill runs its `ensure`. ARGV[0] is
# a file the test made.
path = ARGV[0]
holder = File.open(path, "w")
holder.flock(File::LOCK_EX)

def waiting(thread)
  Thread.pass while thread.status && thread.status != "sleep"
  sleep 0.05
  Thread.pass while thread.status && thread.status != "sleep"
end

raised = Thread.new do
  File.open(path, "w") { |f| f.flock(File::LOCK_EX) }
  :locked
rescue => e
  [e.class, e.message]
end
waiting(raised)
raised.raise(RuntimeError, "boom")
p raised.value

f2 = File.open(path, "w")
closed = Thread.new do
  f2.flock(File::LOCK_EX)
  :locked
rescue IOError => e
  [e.class, e.message]
end
waiting(closed)
f2.close
p closed.value

left = Thread.new do
  File.open(path, "w") { |f| f.flock(File::LOCK_EX) }
ensure
  $stderr.puts "ensure ran"
end
waiting(left)
puts "ending"
