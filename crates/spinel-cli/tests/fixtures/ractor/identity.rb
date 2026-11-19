p Ractor.current == Ractor.main, Ractor.main?
p Ractor.count
r = Ractor.new(name: "worker") do
  [Ractor.current.name, Ractor.main?, Ractor.current == Ractor.main, Ractor.current.inspect]
end
p r.name
p r.value
p r
p r.to_s == r.inspect
p Ractor.main
anon = Ractor.new { }
anon.join
p anon.name, anon
p Ractor.new { Ractor.count }.value
