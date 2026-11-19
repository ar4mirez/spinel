# What may not cross between Ractors raises, and leaves what it refused as
# it was.
Warning[:experimental] = false
def try(label)
  puts "#{label}: #{yield.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.gsub(/0x\h+/, "0x")}"
  puts "  cause: #{e.cause.class}: #{e.cause.message}" if e.cause
end

LP = proc { 45 }
H = { a: [1] }
FROZEN_WITH_PROC = [proc { 1 }].freeze
class C
  define_method(:unshared) { 47 }
end

try("Ractor.new reading outside") { x = 1; Ractor.new { x } }
try("Ractor.new reading two") { x = 1; y = 2; Ractor.new { [x, y] } }
def ractor_yield = Ractor.new { yield }
try("Ractor.new with yield") { ractor_yield {} }
def ractor_write_yield
  x = 1
  Ractor.new { x = 2; yield }
end
try("Ractor.new writing, with yield") { ractor_write_yield {} }
try("Ractor.new self") { Ractor.new { self.class }.value }

try("send a Proc") do
  pr = proc { 1 }
  r = Ractor.new { Ractor.receive }
  begin
    r.send(pr)
  ensure
    r.send(:done)
    puts "  proc still callable: #{pr.call}, frozen: #{pr.frozen?}"
    r.join
  end
end
try("send a frozen Proc") { Ractor.new { Ractor.receive }.send(proc { 1 }.freeze) }
try("send an Array holding a Proc") do
  a = [1, proc { 2 }]
  r = Ractor.new { Ractor.receive }
  begin
    r.send(a)
  ensure
    r.send(:done)
    puts "  sender keeps: #{a.size}, #{a.frozen?}, #{a[1].call}"
    r.join
  end
end
try("unshareable Proc constant") { Ractor.new { LP.call }.value }
try("frozen Array holding a Proc") { Ractor.new { FROZEN_WITH_PROC.size }.value }
try("mutable Hash constant") { Ractor.new { H[:a] }.value }
try("define_method with an unshareable Proc") { Ractor.new { C.new.unshared }.value }
try("constant set to a mutable object") do
  Ractor.new do
    begin
      Object.const_set(:M, [+"m"])
    rescue => e
      [e.class, e.message, Object.const_defined?(:M)]
    end
  end.value
end
try("constant set to a shareable Proc") do
  Ractor.new { Object.const_set(:MP, Ractor.shareable_proc { 7 }); MP.call }.value
end
try("the main Ractor reads it") { MP.call }
try("shareable_proc failing leaves the block") do
  x = +"s"
  pr = proc { x }
  begin
    Ractor.shareable_proc(&pr)
  rescue Ractor::IsolationError => e
    [e.message, pr.frozen?, x.frozen?, pr.call]
  end
end
try("make_shareable copy failing leaves the original") do
  a = [+"x", Thread.current]
  begin
    Ractor.make_shareable(a, copy: true)
  rescue => e
    [e.class, a.frozen?, a[0].frozen?]
  end
end
