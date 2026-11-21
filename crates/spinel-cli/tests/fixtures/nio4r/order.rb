# The order `select` hands ready monitors over in, on the `:poll` and
# `:select` backends (#871), and on `:epoll`, which `order.txt` records
# only where the platform has it. `order.txt` is what CRuby 4.0.9 printed
# with upstream nio4r 2.7.5, its C extension compiled, on x86-64 Linux;
# `crates/spinel-cli/tests/nio4r.rs` runs this on Spinel with nio4r-spinel
# installed in nio4r's place and compares, byte for byte.
#
# libev feeds each ready descriptor's watchers to its pending queue, the
# newest started first, and invokes that queue last fed first. poll(2)
# walks the slots of `polls`, a descriptor's slot taken when its first
# watcher starts and given to the last one's when its last stops; select(2)
# walks the descriptors from the highest down.
require "nio"

backends = %i[poll select]
backends << :epoll if NIO::Selector.backends.include?(:epoll)

backends.each do |backend|
  s = NIO::Selector.new(backend)
  pipes = Array.new(6) { IO.pipe }
  pipes.each { |_, w| w.write("x") }
  name = ->(m) { m.value }
  # Readers, then writers, registered in turn.
  ms = pipes.each_with_index.map do |(r, w), i|
    m = i.even? ? s.register(r, :r) : s.register(w, :w)
    m.value = i
    m
  end
  p [backend, :array, s.select(0).map { |m| [name.(m), m.readiness] }]
  seen = []
  count = s.select(0) { |m| seen << [name.(m), m.readiness] }
  p [backend, :block, count, seen]

  # Both ends of a pipe, and a writer registered before a reader.
  r, w = IO.pipe
  w.write("y")
  late_w = s.register(w, :w); late_w.value = :late_w
  late_r = s.register(r, :r); late_r.value = :late_r
  p [backend, :late, s.select(0).map(&name)]

  # A monitor whose interests change; one deregistered and registered again;
  # one closed.
  ms[1].interests = :rw
  ms[3].interests = :w
  s.deregister(pipes[2][0])
  again = s.register(pipes[2][0], :r); again.value = :again
  ms[4].close
  p [backend, :changed, s.select(0).map(&name)]
  ms[0].add_interest(:w)
  ms[5].remove_interest(:w)
  ms[5].add_interest(:r)
  p [backend, :added, s.select(0).map { |m| [name.(m), m.readiness] }]

  # Two IOs on one descriptor, the second registered later.
  dup = IO.for_fd(pipes[1][1].fileno, autoclose: false)
  twin = s.register(dup, :w); twin.value = :twin
  p [backend, :twin, s.select(0).map(&name)]
  ms[1].interests = :w
  p [backend, :twin_changed, s.select(0).map(&name)]

  # One no longer ready, and one ready again.
  pipes[0][0].read_nonblock(1)
  p [backend, :drained, s.select(0).map(&name)]
  pipes[0][1].write("z")
  p [backend, :refilled, s.select(0).map(&name)]

  # Regular files, always ready, which epoll refuses (`epoll_eperms`).
  files = Array.new(3) { File.open(__FILE__) }
  fs = files.each_with_index.map { |f, i| m = s.register(f, i == 1 ? :w : :r); m.value = :"file#{i}"; m }
  p [backend, :files, s.select(0).map(&name)]
  fs[0].interests = :rw
  s.deregister(files[1])
  p [backend, :files_changed, s.select(0).map(&name)]
  s.register(files[1], :r).value = :file1_again
  p [backend, :files_again, s.select(0).map(&name)]

  # A descriptor closed and taken by a new IO, with the same interests,
  # between two runs: a watcher started on it goes to the backend again
  # (`EV__IOFDSET`), so epoll watches the new IO.
  old_r, old_w = IO.pipe
  old_w.write("a")
  s.register(old_r, :r).value = :old
  p [backend, :before_reuse, s.select(0).map(&name)]
  fd = old_r.fileno
  s.deregister(old_r)
  old_r.close
  new_r, new_w = IO.pipe
  s.register(new_r, :r).value = :reused
  new_w.write("b")
  p [backend, :reused, new_r.fileno == fd, s.select(0).map(&name)]

  # A regular file's descriptor, which epoll refused, taken by a pipe that
  # is not ready, then is.
  file = File.open(__FILE__)
  s.register(file, :r).value = :file
  p [backend, :before_file_reuse, s.select(0).map(&name)]
  fd = file.fileno
  s.deregister(file)
  file.close
  pipe_r, pipe_w = IO.pipe
  s.register(pipe_r, :r).value = :pipe
  p [backend, :file_reused, pipe_r.fileno == fd, s.select(0).map(&name)]
  pipe_w.write("c")
  p [backend, :file_reused_ready, s.select(0).map(&name)]

  s.close
  dup.autoclose = false
  (pipes.flatten + [r, w] + files + [old_w, new_r, new_w, pipe_r, pipe_w]).each(&:close)
end
