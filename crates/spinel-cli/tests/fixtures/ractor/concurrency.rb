Warning[:experimental] = false
class Integer
  def tripled = self * 3
end
workers = 8.times.map do |n|
  Ractor.new(n) do |k|
    sum = 0
    20_000.times do |i|
      s = "item #{i}"
      a = [i, s, { k => s }]
      sum += a[0].tripled + s.size
    end
    [k, sum]
  end
end
200.times { |i| Integer.define_method(:"m#{i}") { i } }
p workers.map(&:value).sort
p 1.m199

port = Ractor::Port.new
pinger = Ractor.new(port) do |out|
  1000.times { |i| out << Ractor.receive + 1 }
  :done
end
total = 0
1000.times do |i|
  pinger << i
  total += port.receive
end
p total, pinger.value

outer = Ractor.new do
  inner = Ractor.new { Ractor.current.inspect.include?("running") }
  [inner.value, Ractor.count >= 2]
end
p outer.value

r = Ractor.new do
  q = Queue.new
  t = Thread.new { q << Ractor.receive * 2 }
  t.join
  q.pop
end
r << 21
p r.value

a = Ractor.new { sleep 0.05; :a }
b = Ractor.new { :b }
got = []
2.times do
  who, value = Ractor.select(a, b)
  got << value
end
p got.sort

mon = Ractor::Port.new
m = Ractor.new { Ractor.receive; raise "boom" rescue nil; :ok }
m.monitor(mon)
m << 1
p mon.receive, m.value
ObjectSpace.each_object(Class).count
GC.start
p :end
