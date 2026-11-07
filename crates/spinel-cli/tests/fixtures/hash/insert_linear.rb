# How the time filling a Hash takes grows with the Hash, for each example in
# #729, which found it quadratic because every store scanned the keys already
# there: String keys, Integer keys, and `ENV.to_h` building a Hash of the
# environment. Each is timed at one size and at eight times it, and the ratio
# is printed: about 8 where a store costs the same whatever the size, several
# times that where it costs the size.

def seconds
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end

# The best of a few, so a pause elsewhere does not count as the store's.
def ratio(name, short, tries = 3)
  times = [short, short * 8].map do |count|
    (1..tries).map { yield(count) }.min
  end
  puts "#{name} #{(times[1] / times[0]).round(1)}"
end

ratio("string-keys", 250) do |n|
  keys = (0...n).map { |i| "k#{i}" }
  seconds { h = {}; i = 0; while i < n; h[keys[i]] = 1; i += 1; end }
end

ratio("integer-keys", 250) do |n|
  seconds { h = {}; i = 0; while i < n; h[i] = 1; i += 1; end }
end

# This process's own environment, emptied and refilled with `n` variables,
# then read whole after a write through `ENV`, which is what makes `to_h`
# build its table again rather than copy the one it kept.
ratio("env-to-h", 40) do |n|
  ENV.clear
  i = 0
  while i < n
    ENV["SPINEL_729_#{i}"] = "v"
    i += 1
  end
  seconds { ENV["SPINEL_729"] = "v"; ENV.to_h }
end
