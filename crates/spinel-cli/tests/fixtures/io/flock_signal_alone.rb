# A lone thread waiting for a lock another process holds (#41): Ctrl-C ends
# the wait with an `Interrupt`, and a trapped signal's handler runs and the
# wait goes on, until SIGTERM ends it. ARGV[0] is the file the test locked;
# ARGV[1] is "int" or "trap".
f = File.open(ARGV[0], "w")
trap("USR1") { STDERR.puts "usr1 handled" } if ARGV[1] == "trap"
begin
  STDERR.puts "locking"
  f.flock(File::LOCK_EX)
  STDERR.puts "locked"
rescue Interrupt => e
  STDERR.puts "interrupted #{e.class}"
end
