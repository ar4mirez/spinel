# A tty goes to `writev` as a sync IO does: a close from a `to_s` with no bytes
# to send leaves the write nothing to raise.
tty = File.open("/dev/tty", "w")
closer = Object.new
closer.define_singleton_method(:to_s) do
  tty.close
  ""
end
answers = [tty.write("", closer)]
tty = File.open("/dev/tty", "w")
writer = Object.new
writer.define_singleton_method(:to_s) do
  tty.write("[nested]")
  tty.close
  ""
end
answers << tty.write("", writer)
File.write(ARGV[0], answers.inspect)
