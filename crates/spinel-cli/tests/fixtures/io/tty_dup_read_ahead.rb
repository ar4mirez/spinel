# A tty is duplex from its open, so `dup` leaves what was read ahead.
f = File.open("/dev/tty", "r+")
c = f.getc
f.dup
line = f.gets
File.write(ARGV[0], [c, line].inspect)
