# On Linux a UNIX socket name starting with a NUL is in the abstract
# namespace: Puma binds `unix://@name` there, and asks whether it can by
# binding "\0puma.temp.unix". The process id keeps two runs apart; it is
# masked in what is printed.
require "socket"

name = "\0spinel.abstract.#{$$}"
show = ->(v) { p(v.is_a?(Array) ? v.map { |x| String === x ? x.sub($$.to_s, "PID") : x } : (String === v ? v.sub($$.to_s, "PID") : v)) }
show.(Socket.sockaddr_un("\0abc").bytesize)
show.(Socket.sockaddr_un("abc").bytesize)
show.(Socket.unpack_sockaddr_un(Socket.sockaddr_un("\0abc")))
server = UNIXServer.new(name)
show.(server.path)
show.(server.addr)
show.(server.local_address.unix_path)
client = UNIXSocket.new(name)
accepted = server.accept
show.(client.peeraddr)
show.(accepted.addr)
client.write "hi"
show.(accepted.read(2))
[client, accepted, server].each(&:close)
begin
  UNIXSocket.new("\0spinel.abstract.none.#{$$}")
rescue SystemCallError => e
  show.([e.class.to_s, e.message])
end
begin
  UNIXServer.new("a\0b")
rescue ArgumentError => e
  show.([e.class.to_s, e.message])
end
