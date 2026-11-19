r = Ractor.new { Ractor.receive.upcase }
s = +"moved"
r.send(s, move: true)
p r.value
begin
  s.size
rescue Ractor::MovedError => e
  p e.class, e.message
end
begin
  s.inspect
rescue Ractor::MovedError => e
  p e.message
end
