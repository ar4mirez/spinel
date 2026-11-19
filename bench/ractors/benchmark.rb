# CPU-bound work split across Ractors (#119): one fixed amount of work, cut
# into as many equal parts as there are Ractors, each part run by a Ractor of
# its own, so the time falls as Ractors are added when they run in parallel.
#
#   target/release/spinel run bench/ractors/benchmark.rb 4      # 4 Ractors
#   ruby bench/ractors/benchmark.rb 4                           # the same on CRuby
#   target/release/spinel run bench/ractors/benchmark.rb 0      # no Ractor at all
#
# A second argument is the number of units, 960 by default; the parts must
# divide it. Each unit is a little of what a Ruby program spends its time on:
# method calls, objects with instance variables, blocks, an Array, a Hash and
# a String. The checksum is the same however the work is cut, which is what
# `crates/spinel-cli/tests/ractor_bench.rs` checks.
#
# It prints the checksum, then the wall time from the first Ractor made to
# the last one's value, in milliseconds: boot is not part of it.
# `bench/ractors.sh` runs it and keeps the numbers; see `bench/README.md`.

Warning[:experimental] = false

class Vec
  attr_reader :x, :y

  def initialize(x, y)
    @x = x
    @y = y
  end

  def +(other) = Vec.new(x + other.x, y + other.y)
  def dot(other) = x * other.x + y * other.y
end

def fib(n) = n < 2 ? n : fib(n - 1) + fib(n - 2)

def unit(seed)
  sum = fib(17)
  points = (1..150).map { |i| Vec.new(i, seed + i) }
  total = points.reduce { |a, b| a + b }
  sum += total.dot(Vec.new(1, 2))
  counts = Hash.new(0)
  points.each { |point| counts[point.x % 7] += point.y }
  sum += counts.values.sum
  text = +""
  40.times { |i| text << (seed + i).to_s }
  sum + text.size
end

def part(first, count)
  sum = 0
  i = first
  while i < first + count
    sum += unit(i)
    i += 1
  end
  sum
end

parts = Integer(ARGV[0] || 4)
units = Integer(ARGV[1] || 960)
abort "#{units} units do not divide into #{parts} parts" if parts > 0 && units % parts != 0

started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
checksum =
  if parts.zero?
    part(0, units)
  else
    size = units / parts
    ractors = (0...parts).map { |k| Ractor.new(k * size, size) { |first, count| part(first, count) } }
    ractors.sum(&:value)
  end
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
puts "checksum: #{checksum}"
puts "ms: #{(elapsed * 1000).round(1)}"
