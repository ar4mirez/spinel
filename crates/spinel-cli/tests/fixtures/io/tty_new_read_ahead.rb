# `IO.new` looks for a tty too.
f = IO.new(IO.sysopen("/dev/tty", "r+"), "r+")
c = f.getc
error = begin
  f.seek(0)
rescue SystemCallError => e
  e.class
end
line = f.gets
File.write(ARGV[0], [c, error, line].inspect)
