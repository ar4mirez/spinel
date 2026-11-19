r = Ractor.new do
  a = Ractor.receive
  b = Ractor.recv
  Ractor.main.send([a, b])
  Ractor.receive * 2
end
r.send(1)
r << 2
p Ractor.receive
r.send(21)
p r.value
port = Ractor::Port.new
w = Ractor.new(port) do |out|
  out << :from_port
  out.send(:again)
end
p port.receive, port.receive
w.join
