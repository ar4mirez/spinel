# A write to the standard output a program was given, a pipe nothing reads,
# which is blocking (#41): it fills the pipe and waits, and a SIGTERM ends
# it. With an argument, another thread sleeps meanwhile.
sleeper = Thread.new { sleep } if ARGV[0] == "thread"
STDERR.puts "writing"
STDOUT.write("x" * 1_000_000)
STDOUT.flush
STDERR.puts "never"
