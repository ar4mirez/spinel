# A child forked from a process that has had threads holds as many
# descriptors as its parent, generation after generation.
$stdout.sync = true
def open_fds = Dir.children("/dev/fd").size

base = open_fds
threads = 3.times.map { Thread.new { sleep 0.1 } }
pid = fork do
  child = open_fds
  2.times.map { Thread.new { sleep 0.05 } }.each(&:join)
  grandchild = IO.pipe.then do |r, w|
    gpid = fork { w.write((open_fds - 2).to_s); w.close; exit!(0) }
    w.close
    Process.wait(gpid)
    Integer(r.read)
  end
  puts [child - base, grandchild - base].inspect
  exit!(0)
end
Process.wait(pid)
threads.each(&:join)
p open_fds - base
