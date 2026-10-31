# A tty has no write buffer: a `to_s` that closes it leaves what the write had
# gathered unwritten, and the write finds the IO closed.
tty = File.open("/dev/tty", "w")
closer = Object.new
closer.define_singleton_method(:to_s) do
  tty.close
  "after"
end
answer =
  begin
    tty.write("gathered", closer)
  rescue IOError => e
    [e.class, e.message]
  end
File.open("/dev/tty", "w") { |f| f.write("all ", "of it") }
File.write(ARGV[0], answer.inspect)
