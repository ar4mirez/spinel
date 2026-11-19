# What a program sees of the nio4r gem's Ruby API: each line one call's
# answer or the exception it raised, class and message. `surface.txt` is what
# CRuby 4.0 printed with upstream nio4r 2.7.5, its C extension compiled
# (`build.rb` records it); `crates/spinel-cli/tests/nio4r.rs` runs this on
# Spinel with nio4r-spinel installed in nio4r's place and compares, line for
# line.
#
# What depends on the machine is printed as whether it is what it should be:
# the backends libev was compiled with past the three Spinel has (`:linuxaio`
# and `:io_uring` on Linux), the queue this platform lacks, each backend by
# its place in `Selector.backends` rather than its name, and object
# addresses, masked. A byte past 127 reads negative where a C `char` is
# signed, as on x86-64 Linux and Apple's arm64, where CI runs this.
require "nio"
require "socket"

# One line, an object's address masked.
def show(label)
  v = yield
  puts "#{label}: #{v.inspect}".gsub(/0x\h{8,}/, "0x")
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.inspect}".gsub(/0x\h{8,}/, "0x")
end

linux = RUBY_PLATFORM.include?("linux")

show("VERSION") { [NIO::VERSION, NIO.engine, NIO.pure?, NIO.pure?("NIO4R_PURE" => "true")] }
show("classes") { [NIO::Selector.superclass, NIO::Monitor.superclass, NIO::ByteBuffer.ancestors.take(3)] }
show("errors") do
  [NIO::ByteBuffer::OverflowError, NIO::ByteBuffer::UnderflowError, NIO::ByteBuffer::MarkUnsetError].map(&:superclass)
end
show("backends") do
  backends = NIO::Selector.backends
  [backends.class, backends.first(3) == (linux ? %i[epoll poll select] : %i[poll kqueue select])]
end
show("default backend") { NIO::Selector.new.backend == (linux ? :epoll : :select) }
show("backend nil") { NIO::Selector.new(nil).backend == NIO::Selector.new.backend }
show("new :derp") { NIO::Selector.new(:derp) }
show("new 42") { NIO::Selector.new(42) }
show("new string") { NIO::Selector.new("epoll") }
show("new other queue") do
  other = linux ? :kqueue : :epoll
  NIO::Selector.new(other)
rescue ArgumentError => e
  [e.class, e.message == "unsupported backend: #{other.inspect}"]
end
show("new 2 args") { NIO::Selector.new(nil, nil) }
show("instance_variables") { [NIO::Selector.new.instance_variables, NIO::Selector.new.register(IO.pipe[0], :r).instance_variables] }
show("inspect") { [NIO::Selector.new.inspect, NIO::Selector.new.register(IO.pipe[0], :r).inspect] }

NIO::Selector.backends.first(3).each.with_index(1) do |backend, i|
  label = "backend #{i}"
  s = NIO::Selector.new(backend)
  r, w = IO.pipe
  show("#{label} backend") { s.backend == backend }
  show("#{label} empty") { [s.empty?, s.registered?(r), s.closed?] }
  m = s.register(r, :r)
  show("#{label} register") { [m.class, m.io.equal?(r), m.selector.equal?(s), m.interests, m.closed?, s.empty?, s.registered?(r)] }
  show("#{label} register again") { s.register(r, :w) }
  show("#{label} readiness before") { [m.readiness, m.readable?, m.writable?, m.writeable?, m.value] }
  show("#{label} select 0, none ready") { s.select(0) }
  show("#{label} select 0.05, none ready") { s.select(0.05) }
  w << "hi"
  show("#{label} select 0") { s.select(0).map { |x| [x.equal?(m), x.readiness, x.readable?, x.writable?] } }
  show("#{label} select nil") { s.select.size }
  show("#{label} select block") { got = []; [s.select(1) { |x| got << x.readiness }, got] }
  r.read_nonblock(2)
  show("#{label} select drained") { [s.select(0), m.readiness] }
  wm = s.register(w, :rw)
  show("#{label} writer") { [s.select(0).map(&:readiness), wm.interests] }
  show("#{label} interests=") { [wm.interests = :r, wm.interests, s.select(0), wm.readiness] }
  show("#{label} interests= nil") { [(wm.interests = nil), wm.interests] }
  show("#{label} add_interest") { [wm.add_interest(:w), wm.add_interest(:r), wm.interests] }
  show("#{label} remove_interest") { [wm.remove_interest(:r), wm.remove_interest(:r), wm.remove_interest(:w), wm.interests] }
  show("#{label} bad interests") { wm.interests = :x }
  show("#{label} string interest") { wm.add_interest("r") }
  show("#{label} integer interest") { wm.remove_interest(1) }
  show("#{label} bad add") { wm.add_interest(:derp) }
  show("#{label} value") { [wm.value = 42, wm.value] }
  show("#{label} deregister") { d = s.deregister(w); [d.equal?(wm), d.closed?, s.registered?(w), s.deregister(w)] }
  show("#{label} closed monitor interests=") { wm.interests = :rw }
  show("#{label} closed monitor add_interest") { wm.add_interest(:r) }
  show("#{label} wakeup") { s.wakeup }
  show("#{label} select after wakeup") { s.select(1) }
  t = Thread.new { s.select }
  Thread.pass until t.status == "sleep" || !t.alive?
  sleep 0.05
  s.wakeup
  show("#{label} wakeup from another thread") { t.value }
  t = Thread.new { s.select { |x| x } }
  Thread.pass until t.status == "sleep" || !t.alive?
  sleep 0.05
  s.wakeup
  show("#{label} wakeup with a block") { t.value }
  w.close
  show("#{label} peer closed") { s.select(1).map(&:readiness) }
  show("#{label} monitor close") { [m.close, m.closed?, s.registered?(r), m.close, m.selector] }
  r2, w2 = IO.pipe
  m2 = s.register(r2, :r)
  r2.close
  show("#{label} deregister closed io") { s.deregister(r2).equal?(m2) }
  show("#{label} register closed io") { s.register(r2, :r) }
  r3, = IO.pipe
  m3 = s.register(r3, :r)
  show("#{label} close") { [s.close, s.closed?, s.close, m3.closed?] }
  show("#{label} closed backend") { s.backend }
  show("#{label} closed register") { s.register(IO.pipe[0], :r) }
  show("#{label} closed select") { s.select(0) }
  show("#{label} closed wakeup") { s.wakeup }
  show("#{label} closed deregister") { [s.deregister(r).nil?, s.registered?(r3), s.empty?] }
  show("#{label} monitor close after selector") { [m3.close, m3.closed?, s.registered?(r3)] }
  w2.close
end

s = NIO::Selector.new
r, w = IO.pipe
show("register 42") { s.register(42, :r) }
show("register nil") { s.register(nil, :r) }
show("register string interest") { s.register(r, "r") }
show("register integer interest") { s.register(r, 1) }
show("register bad interest") { s.register(r, :x) }
to_io = Object.new
def to_io.to_io = 5
show("register to_io non-IO") { s.register(to_io, :r) }
wrapped = Object.new
wrapped.define_singleton_method(:to_io) { r }
show("register to_io") { m = s.register(wrapped, :r); [m.io.equal?(wrapped), s.registered?(wrapped), s.registered?(r)] }
show("select string") { s.select("1") }
show("select symbol") { s.select(:a) }
show("select true") { s.select(true) }
show("select object") { s.select(Object.new) }
show("select negative") { s.select(-0.5) }
show("select negative integer") { s.select(-1) }
show("select rational") { s.select(1r / 100) }
show("select 2 args") { s.select(1, 2) }
show("monitor close 2 args") { s.register(w, :w).close(1, 2) }
show("monitor close false") { m = s.register(IO.pipe[1], :w); [m.close(false), m.closed?, s.registered?(m.io)] }
show("Monitor.new") { NIO::Monitor.new(r, :x, s) }
s.close

show("ByteBuffer.new") { b = NIO::ByteBuffer.new(8); [b.capacity, b.size, b.position, b.limit, b.remaining, b.full?] }
show("ByteBuffer.new symbol") { NIO::ByteBuffer.new(:symbols_are_bogus) }
show("ByteBuffer.new nil") { NIO::ByteBuffer.new(nil) }
show("ByteBuffer.new string") { NIO::ByteBuffer.new("3") }
show("ByteBuffer.new float") { NIO::ByteBuffer.new(2.7).capacity }
show("ByteBuffer.new big") { NIO::ByteBuffer.new(2**40) }
show("ByteBuffer.new no args") { NIO::ByteBuffer.new }
b = NIO::ByteBuffer.new(16)
show("inspect") { b.inspect }
show("<<") { (b << "Hello" << ", ").equal?(b) }
show("<< position") { [b.position, b.limit, b.remaining] }
show("<< integer") { b << 1 }
show("<< nil") { b << nil }
show("<< to_str") { o = Object.new; def o.to_str = "ab"; (b << o).position }
show("<< multibyte") { (b << "é").position }
show("<< overflow") { b << "X" * 20 }
show("flip") { [b.flip.equal?(b), b.position, b.limit] }
show("get 3") { v = b.get(3); [v, v.encoding.to_s, v.frozen?] }
show("get rest") { v = b.get; [v, v.encoding.to_s] }
show("get empty") { [b.get, b.get(0)] }
show("get more") { b.get(1) }
show("get negative") { b.get(-1) }
show("get nil") { b.get(nil) }
show("get float") { b.rewind.get(2.9) }
show("[]") { [b[0], b[1], b[b.limit - 1]] }
show("[] negative") { b[-1] }
show("[] limit") { b[b.limit] }
show("[] string") { b["1"] }
show("each") { got = []; [b.each { |x| got << x }.equal?(b), got] }
show("each no block") { b.each }
show("to_a") { b.to_a }
show("map") { b.map { |x| x + 1 }.size }
show("position=") { [b.position = 2, b.position] }
show("position= float") { [b.position = 1.9, b.position] }
show("position= negative") { b.position = -1 }
show("position= beyond limit") { b.position = b.limit + 1 }
show("limit=") { [b.limit = 5, b.limit, b.position] }
show("limit= negative") { b.limit = -1 }
show("limit= beyond capacity") { b.limit = 17 }
show("limit= capacity") { [b.limit = 16, b.limit] }
show("mark") { b.position = 3; [b.mark.equal?(b), b.position] }
show("reset") { b.position = 6; [b.reset.position] }
show("mark cleared by position") { b.mark; b.position = 1; b.reset }
show("mark cleared by limit") { b.position = 4; b.mark; b.limit = 2; b.reset }
show("mark cleared by flip") { b.mark; b.flip; b.reset }
show("mark cleared by rewind") { b.mark; b.rewind; b.reset }
show("mark cleared by clear") { b.mark; b.clear; b.reset }
show("clear") { [b.clear.equal?(b), b.position, b.limit, b.get(4)] }
show("compact") do
  c = NIO::ByteBuffer.new(12)
  c << "CompactMe" << "Left"
  c.position = 7
  c.limit = 11
  [c.compact.equal?(c), c.position, c.limit, c.flip.get]
end
show("full") { f = NIO::ByteBuffer.new(3); f << "abc"; [f.full?, f.remaining] }
show("bytes past 127") { f = NIO::ByteBuffer.new(3); f << "\xff\x80a".b; [f[0], f[1], f[2], f.to_a] }
show("instance_variables") { NIO::ByteBuffer.new(1).instance_variables }

r, w = IO.pipe
b = NIO::ByteBuffer.new(8)
show("read_from empty") { b.read_from(r) }
w << "abcdefghij"
show("read_from") { [b.read_from(r), b.position, b.full?] }
show("read_from full") { b.read_from(r) }
show("read_from 42") { b.read_from(42) }
show("read_from closed") { x, = IO.pipe; x.close; NIO::ByteBuffer.new(1).read_from(x) }
show("write_to") { b.flip; [b.write_to(w), b.position, r.read_nonblock(100)] }
show("write_to empty") { b.write_to(w) }
show("write_to 42") { b.flip; b.write_to(42) }
w.close
show("read_from eof") { x = NIO::ByteBuffer.new(4); [r.read_nonblock(100), x.read_from(r), x.position] }
show("write_to closed peer") { x, y = IO.pipe; x.close; z = NIO::ByteBuffer.new(2); z << "ab"; z.flip; z.write_to(y) }

server = TCPServer.new("127.0.0.1", 0)
client = TCPSocket.new("127.0.0.1", server.local_address.ip_port)
peer = server.accept
s = NIO::Selector.new
cm = s.register(client, :rw)
pm = s.register(peer, :r)
show("tcp writable") { s.select(1).map { |x| [x.equal?(cm), x.readiness] } }
client.write("ping")
show("tcp readable") { s.select(1).map { |x| [x.equal?(cm) ? :client : :peer, x.readiness] }.sort }
show("tcp buffer") { x = NIO::ByteBuffer.new(4); [x.read_from(peer), x.flip.get] }
# Deregistered first: what a backend reports for a descriptor closed while
# registered depends on the platform (epoll drops it, select reports it).
s.deregister(client)
client.close
show("tcp hangup") { s.select(1).map { |x| [x.equal?(pm), x.readiness] } }
s.close
