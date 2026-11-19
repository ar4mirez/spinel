# `Ractor.make_shareable`: a deep freeze, with CRuby's refusals and its
# checks of a `Proc`.
Warning[:experimental] = false
class Holder
  attr_reader :value
  def initialize(value) = @value = value
end
def try(label)
  puts "#{label}: #{yield.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.gsub(/0x\h+/, "0x")}"
end

try("deep") do
  a = [+"x", { b: +"y" }, Holder.new([+"z"])]
  same = Ractor.make_shareable(a).equal?(a)
  [same, a.frozen?, a[0].frozen?, a[1].frozen?, a[1][:b].frozen?, a[2].frozen?, a[2].value.frozen?,
   a[2].value[0].frozen?, Ractor.shareable?(a)]
end
try("copy") do
  a = [+"x"]
  c = Ractor.make_shareable(a, copy: true)
  [c.equal?(a), a.frozen?, a[0].frozen?, c.frozen?, c[0].frozen?, c == a]
end
try("copy of shareable") { s = "x".freeze; Ractor.make_shareable(s, copy: true).equal?(s) }
try("copy Struct") do
  s = Struct.new(:a).new(+"x")
  c = Ractor.make_shareable(s, copy: true)
  [c.equal?(s), s.frozen?, c.frozen?, c.a.frozen?]
end
try("copy Proc") { Ractor.make_shareable(proc {}, copy: true) }
try("copy Thread") { Ractor.make_shareable([Thread.current], copy: true) }
try("Class") { Ractor.make_shareable(String) }
try("Ractor") { Ractor.make_shareable(Ractor.current).equal?(Ractor.current) }
try("Thread") { Ractor.make_shareable(Thread.current) }
try("Mutex") { Ractor.make_shareable(Mutex.new) }
try("Queue") { Ractor.make_shareable(Queue.new) }
try("Enumerator") { Ractor.make_shareable([1].each) }
try("Array of Thread") { Ractor.make_shareable([Thread.current]) }
try("frozen before the refusal") do
  a = [+"a", Thread.current, +"b"]
  Ractor.make_shareable(a) rescue nil
  [a.frozen?, a[0].frozen?, a[2].frozen?]
end
try("unknown keyword") { Ractor.make_shareable([], foo: 1) }

# A Proc is checked: its self, then what it uses outside itself.
try("Proc, main self") { Ractor.make_shareable(proc { 1 }) }
try("Proc, frozen, main self") { Ractor.make_shareable(proc { 1 }.freeze) }
try("Proc in an Array") { Ractor.make_shareable([proc { 1 }]) }
try("Hash default proc") { Ractor.make_shareable(Hash.new { 1 }) }
try("Proc, nil self") do
  pr = nil.instance_eval { proc { 1 } }
  Ractor.make_shareable(pr)
  [pr.frozen?, Ractor.shareable?(pr), pr.call]
end
try("lambda, nil self") do
  pr = nil.instance_eval { -> { 2 } }
  Ractor.make_shareable(pr)
  [pr.lambda?, Ractor.shareable?(pr), pr.call]
end
try("reads shareable") do
  x = 3
  pr = nil.instance_eval { proc { x } }
  Ractor.make_shareable(pr).call
end
try("reads mutable") do
  x = +"s"
  pr = nil.instance_eval { proc { x } }
  begin
    Ractor.make_shareable(pr)
  ensure
    puts "  x frozen: #{x.frozen?}, proc shareable: #{Ractor.shareable?(pr)}"
  end
end
try("reads a Thread") do
  x = Thread.current
  Ractor.make_shareable(nil.instance_eval { proc { x } })
end
try("writes") do
  x = 1
  Ractor.make_shareable(nil.instance_eval { proc { x = 2 } })
end
try("reads reassigned") do
  x = 1
  x = 2
  Ractor.make_shareable(nil.instance_eval { proc { x } })
end
try("self frozen Object") do
  o = Object.new.freeze
  o.instance_eval { Ractor.make_shareable(proc { 4 }) }.call
end
