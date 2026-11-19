# `Ractor.shareable_proc` and `Ractor.shareable_lambda`, and a shareable
# Proc going to another Ractor.
Warning[:experimental] = false
def try(label)
  puts "#{label}: #{yield.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.gsub(/0x\h+/, "0x")}"
end

try("no block") { Ractor.shareable_proc }
try("self is nil") do
  pr = Ractor.shareable_proc { self }
  [pr.frozen?, Ractor.shareable?(pr), pr.lambda?, pr.call]
end
try("self:") do
  pr = Ractor.shareable_proc(self: 5) { self + 1 }
  [pr.call, Ractor.shareable?(pr)]
end
try("self: nil") { Ractor.shareable_proc(self: nil) { self }.call }
try("self: mutable") { Ractor.shareable_proc(self: +"x") { self } }
try("unknown keyword") { Ractor.shareable_proc(foo: 1) {} }
try("arguments") { Ractor.shareable_proc { |a, b| [a, b] }.call(1, 2) }
try("lambda") do
  pr = Ractor.shareable_lambda { |a| a }
  [pr.lambda?, pr.call(3), Ractor.shareable?(pr)]
end
try("lambda self:") { Ractor.shareable_lambda(self: :s) { self }.call }
try("lambda no block") { Ractor.shareable_lambda }
try("from a Proc") do
  pr = proc { 1 }
  sp = Ractor.shareable_proc(&pr)
  [sp.equal?(pr), pr.frozen?, sp.frozen?, Ractor.shareable?(pr), sp.call]
end
try("from a lambda") { Ractor.shareable_proc(&->(a) { a }).lambda? }

# What it reads outside itself.
try("reads once-assigned") do
  x = 1
  pr = Ractor.shareable_proc { x + 1 }
  pr.call
end
try("reads assigned twice") do
  x = 1
  x = 2
  Ractor.shareable_proc { x }
end
try("reads assigned after") do
  x = 1
  pr = Ractor.shareable_proc { x }
  x = 3
  pr.call
end
try("reads op-assigned") do
  x = 1
  x += 1
  Ractor.shareable_proc { x }
end
try("reads assigned in a block") do
  x = 1
  [1].each { x = 5 }
  Ractor.shareable_proc { x }
end
try("reads a block parameter") { [7].map { |v| Ractor.shareable_proc { v }.call } }
try("reads one per iteration") { 2.times.map { |i| x = i; Ractor.shareable_proc { x }.call } }
def param(a) = Ractor.shareable_proc { a }.call
try("reads a parameter") { param(9) }
def reparam(a)
  a = 2
  Ractor.shareable_proc { a }.call
end
try("reads a reassigned parameter") { reparam(9) }
def optional(a = 1) = Ractor.shareable_proc { a }.call
try("reads an optional parameter") { optional }
def keyword(a: 1) = Ractor.shareable_proc { a }.call
try("reads a keyword") { keyword }
try("reads mutable") do
  x = +"s"
  Ractor.shareable_proc { x }
end
try("reads mutable in a nested block") do
  x = []
  Ractor.shareable_proc { [1].map { x } }
end
try("reads frozen") do
  x = "a".freeze
  Ractor.shareable_proc { x }.call
end
try("writes") do
  x = 1
  Ractor.shareable_proc { x = 2 }
end
try("writes two") do
  x = 1
  y = 2
  Ractor.shareable_proc { x = 1; y = 2 }
end
def yields = Ractor.shareable_proc { yield }
try("yield") { yields { 1 } }
def writes_and_yields
  x = 1
  Ractor.shareable_proc { x = 2; yield }
end
try("writes and yield") { writes_and_yields { 1 } }
try("own locals") { Ractor.shareable_proc { z = 1; z + 1 }.call }
try("shadowing parameter") do
  x = +"a"
  Ractor.shareable_proc { |x| x }.call(1)
end
try("Method#to_proc") { Ractor.shareable_proc(&1.method(:+)) }

# To another Ractor.
SP = Ractor.shareable_proc { 44 }
LP = proc { 45 }
class C
  define_method(:shared, &Ractor.shareable_proc { 46 })
  define_method(:unshared) { 47 }
end
try("sent") do
  r = Ractor.new { Ractor.receive.call }
  r.send(Ractor.shareable_proc { 42 })
  r.value
end
try("argument") { Ractor.new(Ractor.shareable_proc(self: 2) { self * 21 }) { |pr| pr.call }.value }
try("reads outside, in another Ractor") do
  base = 40
  pr = Ractor.shareable_lambda { |n| base + n }
  Ractor.new(pr) { |l| [l.lambda?, l.call(3), Ractor.shareable?(l)] }.value
end
try("shareable Proc constant") { Ractor.new { SP.call }.value }
try("define_method with a shareable Proc") { Ractor.new { C.new.shared }.value }
try("made back in another Ractor") do
  Ractor.new { Ractor.shareable_proc { 1 }.call + Ractor.make_shareable(nil.instance_eval { proc { 2 } }).call }.value
end
