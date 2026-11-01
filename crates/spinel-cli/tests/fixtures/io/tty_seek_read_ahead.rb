# A tty is duplex from its open, so a seek that fails leaves what was read ahead.
f = File.open("/dev/tty", "r+")
c = f.getc
error = begin
  f.seek(0)
rescue SystemCallError => e
  e.class
end
line = f.gets
File.write(ARGV[0], [c, error, line].inspect)
