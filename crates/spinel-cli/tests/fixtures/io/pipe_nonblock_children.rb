# Which descriptors are nonblocking: a pipe's ends, not the standard streams
# nor a file. A child's standard streams are blocking, whether it was given
# them or inherited them, and the parent's descriptor for the same pipe end
# loses the flag with it; a descriptor above 2, or a `fork`, keeps it.
require "io/nonblock"
require "rbconfig"
CHILD = %q{require "io/nonblock"; STDERR.puts [0, 1, 2].map { |fd| IO.for_fd(fd, autoclose: false).nonblock? }.inspect}
STDERR.reopen(STDOUT)
STDOUT.sync = true
r, w = IO.pipe
p [:pipe, r.nonblock?, w.nonblock?, r.dup.nonblock?]
p [:stdio, STDIN.nonblock?, STDOUT.nonblock?]
p [:file, File.open(__FILE__) { |f| f.nonblock? }]
io = IO.popen([RbConfig.ruby, "-e", CHILD], "r+", err: [:child, :out])
p [:popen, io.nonblock?, io.read]
io.close
r, w = IO.pipe
system(RbConfig.ruby, "-e", CHILD, in: r)
p [:system_in, r.nonblock?, w.nonblock?]
r, w = IO.pipe
pid = spawn(RbConfig.ruby, "-e", "", 3 => w)
Process.wait(pid)
p [:spawn_fd3, w.nonblock?]
r, w = IO.pipe
STDIN.reopen(r)
Process.wait(fork { exit!(0) })
p [:fork, STDIN.nonblock?]
system(RbConfig.ruby, "-e", CHILD)
p [:inherited, STDIN.nonblock?, r.nonblock?]
w.nonblock = true
p [:set, r.nonblock?, w.nonblock?]
w.nonblock = false
p [:cleared, w.nonblock?]
