# A write to a pipe nothing reads, alone: it fills the pipe and waits, and a
# SIGTERM ends it.
r, w = IO.pipe
STDERR.puts "writing"
w.write("x" * 1_000_000)
STDERR.puts "never"
