# Two threads, each reading a child's output that takes 1s to arrive: the
# waits overlap, so both finish in about 1s rather than 2s.
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
readers = 2.times.map do |i|
  Thread.new { IO.popen(["sh", "-c", "sleep 1; echo #{i}"]) { |io| io.read } }
end
p readers.map(&:value)
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
puts elapsed < 1.8 ? "overlapped" : "serial: #{elapsed}"

# A thread that never waits does not starve the other.
counter = 0
busy = Thread.new { loop { counter += 1 } }
sleep 0.05
busy.kill
busy.join
puts counter > 0 ? "preempted" : "starved"
