# A write to a terminal leaves what was read ahead, as CRuby's `FMODE_DUPLEX`.
f = File.open("/dev/tty", "r+")
c = f.getc
f.write("X")
line = f.gets
File.write(ARGV[0], [c, line].inspect)
