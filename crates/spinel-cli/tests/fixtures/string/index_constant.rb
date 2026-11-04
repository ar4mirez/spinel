# How the time a `String#index` or `String#byteindex` whose match is at the
# start grows with the String it searches, the calls #617 found costing the
# whole String: each is timed on a String and on one a hundred times as long,
# and the ratio is printed, about 1 where a call costs what its search does,
# tens where it costs the String's length.

def seconds
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end

# The best of a few, so a pause elsewhere does not count as the call's.
def ratio(name, calls = 200, tries = 3)
  times = [1_000, 100_000].map do |n|
    s = "x" * n + "\n"
    (1..tries).map { seconds { calls.times { yield s } } }.min
  end
  puts "#{name} #{(times[1] / times[0]).round(1)}"
end

ratio("index") { |s| s.index("x", 0) }
ratio("byteindex") { |s| s.byteindex("x", 0) }
