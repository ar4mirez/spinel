# How the time draining or filling an Array from the front takes grows with
# the Array, for each pattern #733 names, which found `shift` quadratic
# because every call copied the rest of the array down: filling an array and
# shifting it empty, a `while (x = a.shift)` loop, a work list that pushes at
# the back while it shifts from the front, `unshift`, and `shift(n)`. Each is
# timed at one size and at eight times it, and the ratio is printed: about 8
# where a shift costs the same whatever the size, several times that where it
# costs the size.

def seconds
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end

# The best of a few, so a pause elsewhere does not count as the shift's.
def ratio(name, short, tries = 3)
  times = [short, short * 8].map do |count|
    (1..tries).map { yield(count) }.min
  end
  puts "#{name} #{(times[1] / times[0]).round(1)}"
end

ratio("fill-then-shift", 1000) do |n|
  seconds { a = []; n.times { |i| a << i }; n.times { a.shift } }
end

ratio("shift-loop", 1000) do |n|
  a = Array.new(n, 1)
  seconds { while (x = a.shift); end }
end

# Breadth first over a binary tree of n nodes: each shift pushes two more.
ratio("work-list", 1000) do |n|
  seconds do
    work = [0]
    while (node = work.shift)
      child = node * 2 + 1
      work << child if child < n
      work << child + 1 if child + 1 < n
    end
  end
end

ratio("unshift", 1000) do |n|
  seconds { a = []; i = 0; while i < n; a.unshift(i); i += 1; end }
end

ratio("shift-n", 1000) do |n|
  a = Array.new(n, 1)
  seconds { while a.shift(2).size > 0; end }
end
