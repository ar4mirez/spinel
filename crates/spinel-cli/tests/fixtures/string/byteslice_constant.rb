# How the time a long slice takes grows with its length, the calls #619 found
# costing their length twice over: each takes all of a String but its first
# byte, on a String and on one a thousand times as long, and the ratio is
# printed, about 1 where a slice shares its source's bytes, hundreds where it
# copies them. `delete_prefix` is `__byteslice__`, the primitive `core/`
# slices with. A BINARY String, so `[]` has no characters to count.

def seconds
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end

# The best of a few, so a pause elsewhere does not count as the call's.
def ratio(name, calls = 2_000, tries = 3)
  times = [1_000, 1_000_000].map do |n|
    s = ("x" * n).b
    (1..tries).map { seconds { calls.times { yield s, n } } }.min
  end
  puts "#{name} #{(times[1] / times[0]).round(1)}"
end

ratio("byteslice") { |s, n| s.byteslice(1, n - 1) }
ratio("[]") { |s, n| s[1, n - 1] }
ratio("delete_prefix") { |s, _| s.delete_prefix("x") }
