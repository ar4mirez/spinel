# A thread whose sleep is over gets a turn while the main thread runs a loop
# that is nearly all core library (#728): `Time.now` is Ruby in
# `core/time.rb`, so a turn check a slice apart can land in it every time.
# The inner loop is the issue's, as it is: another body aliases differently.
def clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)

start = clock
woke = nil
Thread.new { sleep 0.2; woke = clock - start }
loop do
  x = 0
  while x < 5000; Time.now; x += 1; end
  break if woke || clock - start > 5
end
puts(woke && woke < 0.5 ? "woken" : "not woken: #{woke.inspect} after #{clock - start}")
