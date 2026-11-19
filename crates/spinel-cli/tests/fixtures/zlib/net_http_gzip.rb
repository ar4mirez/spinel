# Net::HTTP inflates a gzip body through its inflate_adapter, whose block
# clears the chunk it handed `Zlib::Inflate#inflate`. Bodies around and well
# above zlib.c's 16 KiB yield size come back whole, from a server on
# 127.0.0.1 in this process: no network.
require "net/http"
require "socket"
require "zlib"

def body(n)
  x = n
  out = "".b
  n.times do
    x = (x * 214013 + 2531011) & 0x7fffffff
    out << "abcdefghijklmnopqrstuvwxyz \n".getbyte((x >> 16) % 28)
  end
  out
end

SIZES = [1_000, 16_383, 16_384, 16_385, 40_000, 300_000]
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
thread = Thread.new do
  SIZES.size.times do
    client = server.accept
    request = client.gets
    nil until client.gets == "\r\n"
    n = Integer(request[/\/(\d+)/, 1])
    gz = Zlib.gzip(body(n))
    client.write("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n" \
                 "Content-Encoding: gzip\r\nContent-Length: #{gz.bytesize}\r\n" \
                 "Connection: close\r\n\r\n")
    # Written in pieces, so the client reads the body in several chunks.
    (0...gz.bytesize).step(4096) { |i| client.write(gz.byteslice(i, 4096)) }
    client.close
  end
end

SIZES.each do |n|
  res = Net::HTTP.get_response(URI("http://127.0.0.1:#{port}/#{n}"))
  got = res.body
  puts "#{n} #{res.code} #{got.bytesize} #{got == body(n)} #{res["content-encoding"].inspect}"
end
thread.join
server.close
