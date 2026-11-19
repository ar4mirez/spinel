port = Ractor::Port.new
port << 1
p port.closed?
p port.receive
port << 2
port.close
p port.closed?
begin
  port << 3
rescue Ractor::ClosedError => e
  p e.class, e.message
end
p port.receive
begin
  port.receive
rescue Ractor::ClosedError => e
  p e.class, e.message
end
