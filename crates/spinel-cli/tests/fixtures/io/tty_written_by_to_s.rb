# A tty has no write buffer: a write from a `to_s` goes ahead of what the write
# had gathered, and a close after it leaves that unwritten.
tty = File.open("/dev/tty", "w")
writer = Object.new
writer.define_singleton_method(:to_s) do
  tty.write("[nested]")
  "[after]"
end
closer = Object.new
closer.define_singleton_method(:to_s) do
  tty.write("[again]")
  tty.close
  "[after]"
end
tty.write("[gathered]", writer)
answer =
  begin
    tty.write("[unwritten]", closer)
  rescue IOError => e
    [e.class, e.message]
  end
File.write(ARGV[0], answer.inspect)
