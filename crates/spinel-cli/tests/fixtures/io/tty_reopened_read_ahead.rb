# Only an open looks for a tty: `reopen` onto one leaves the IO as it was, so
# a seek that fails discards what was read ahead, the rest of the line here.
f = File.open(__FILE__)
f.reopen("/dev/tty", "r")
c = f.getc
error = begin
  f.seek(0)
rescue SystemCallError => e
  e.class
end
line = f.gets
File.write(ARGV[0], [c, error, line].inspect)
