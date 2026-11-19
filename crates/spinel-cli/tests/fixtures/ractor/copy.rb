s = +"text"
a = [1, [2, 3]]
h = { k: +"v" }
r = Ractor.new do
  str, arr, hash = Ractor.receive
  str << "!"
  arr[1] << 4
  hash[:k] << "?"
  hash[:n] = 1
  [str, arr, hash]
end
r.send([s, a, h])
p r.value
p s, a, h
fz = "frozen".freeze
r2 = Ractor.new { x = Ractor.receive; [x, x.frozen?] }
r2.send(fz)
p r2.value
r3 = Ractor.new { Ractor.receive }
r3.send([1, :sym, nil, true, 2**70, 1.5, 1..3, "a".."c"])
p r3.value
Point = Struct.new(:x, :y)
r4 = Ractor.new { pt = Ractor.receive; pt.x += 1; pt }
pt = Point.new(1, 2)
r4.send(pt)
p r4.value, pt
