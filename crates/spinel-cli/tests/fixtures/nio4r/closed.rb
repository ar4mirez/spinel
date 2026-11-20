# An IO closed while registered with a selector on the `:poll` and `:select`
# backends, never deregistered, then selected (#864). `closed.txt` is what
# CRuby 4.0.9 printed with upstream nio4r 2.7.5, its C extension compiled,
# on x86-64 Linux; `crates/spinel-cli/tests/nio4r.rs` runs this on Spinel
# with nio4r-spinel installed in nio4r's place and compares, byte for byte.
#
# libev kills a descriptor that is not open (`fd_kill`): each of its
# watchers is stopped and handed one last event of reading and writing
# whatever its interests, so `readiness` is `:rw`, and a later `select`
# leaves it out. On `:select`, select(2) fails with EBADF, so that run
# reports the killed monitors and nothing else; poll(2) reports the rest
# beside them. Where several monitors are ready, they are sorted, since the
# order a run hands them over in is not what this checks.
require "nio"

show = lambda do |monitors|
  monitors.map { |m| [m.readiness, m.readable?, m.writable?, m.closed?, m.interests] }
end

%i[poll select].each do |backend|
  %i[r w rw].each do |interests|
    %i[reader writer].each do |side|
      %i[array block].each do |mode|
        s = NIO::Selector.new(backend)
        r, w = IO.pipe
        io = side == :reader ? r : w
        other = side == :reader ? w : r
        m = s.register(io, interests)
        before = [m.readiness, m.readable?, m.writable?]
        io.close
        first = if mode == :array
          got = s.select(0)
          got && show.(got)
        else
          seen = []
          count = s.select(0) { |x| seen << x }
          [count, show.(seen)]
        end
        second = s.select(0)
        p [backend, interests, side, mode, before, first, second && show.(second),
           [m.readiness, m.readable?, m.writable?, m.closed?, m.interests, s.registered?(io)]]
        other.close
        s.close
      end
    end
  end
  # A closed IO beside an open one that is ready.
  s = NIO::Selector.new(backend)
  r1, w1 = IO.pipe
  r2, w2 = IO.pipe
  m1 = s.register(r1, :r)
  m2 = s.register(w2, :w)
  r1.close
  ready = s.select(0)
  p [backend, :beside, ready.map { |x| [x.equal?(m1), x.readiness] }.sort_by { |a| a[0] ? 0 : 1 }]
  # Closed after a select already saw it ready.
  r3, w3 = IO.pipe
  m3 = s.register(r3, :r)
  w3.write("x")
  p [backend, :seen, s.select(0).map { |x| x.equal?(m3) ? x.readiness : nil }.compact]
  r3.close
  p [backend, :then_closed, (s.select(0) || []).map { |x| [x.equal?(m3), x.readiness] }.sort_by { |a| a[0] ? 0 : 1 }]
  # A timed select returns at once.
  r4, w4 = IO.pipe
  m4 = s.register(r4, :r)
  r4.close
  t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  got = s.select(2)
  p [backend, :timed, (got || []).select { |x| x.equal?(m4) }.map(&:readiness), Process.clock_gettime(Process::CLOCK_MONOTONIC) - t < 1]
  # interests changed on a stopped monitor, and deregistered.
  begin; m4.interests = :w; rescue => e; p e; end
  p [backend, :changed, m4.interests, (s.select(0) || []).select { |x| x.equal?(m4) }.map(&:readiness)]
  s.deregister(r4)
  p [backend, :deregistered, s.registered?(r4), m4.closed?]
  [w1, r2, w2, w3, w4].each(&:close)
  s.close
end
