# What reflection sees of the socket classes over loopback: nothing, as on
# CRuby, where a socket is an IO with its state in the T_FILE struct and an
# Addrinfo is a T_DATA. `sockets.out` is what ruby 4.0.9 printed. A machine
# that cannot open a loopback socket prints "unavailable", which the test
# reads as a skip.
require "socket"

begin
  server = TCPServer.new("127.0.0.1", 0)
rescue SystemCallError, SocketError => e
  puts "unavailable: #{e.class}"
  exit
end
client = TCPSocket.new("127.0.0.1", server.addr[1])
accepted = server.accept

listener = Socket.new(:INET, :STREAM)
listener.bind(Addrinfo.tcp("127.0.0.1", 0))
listener.listen(1)
dialer = Socket.new(:INET, :STREAM)
dialer.connect(listener.local_address)
peer, peer_address = listener.accept

puts "TCPServer: #{server.instance_variables.inspect}"
puts "TCPSocket client: #{client.instance_variables.inspect}"
puts "TCPSocket accepted: #{accepted.instance_variables.inspect}"
puts "Socket listener: #{listener.instance_variables.inspect}"
puts "Socket dialer: #{dialer.instance_variables.inspect}"
puts "Socket peer: #{peer.instance_variables.inspect}"
puts "Addrinfo: #{peer_address.instance_variables.inspect}"

# The state is still there, only out of sight.
client.write("ping")
puts accepted.read(4)
dialer.write("pong")
puts peer.read(4)
puts peer_address.ipv4_loopback?

[client, accepted, server, dialer, peer, listener].each(&:close)
