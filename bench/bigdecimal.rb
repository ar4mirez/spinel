# The built-in bigdecimal (#109): money-sized sums and products, division
# to its default precision, rounding, parsing and printing, and the Ruby
# half's arbitrary-precision work, sqrt and BigMath.PI.
#
#   target/release/spinel run bench/bigdecimal.rb
#   ruby --yjit bench/bigdecimal.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "bigdecimal"
require "bigdecimal/math"

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(2)} ms"
end

prices = (1..1000).map { |i| BigDecimal("#{i}.#{i % 100}") }
rate = BigDecimal("0.0825")
texts = prices.map(&:to_s)

time("10k sums                  ") { 10.times { prices.sum(BigDecimal("0")) } }
time("10k products, round(2)    ") { 10.times { prices.each { |p| (p * rate).round(2, :half_even) } } }
time("10k divisions (32 digits) ") { 10.times { prices.each { |p| p / 7 } } }
time("10k BigDecimal(String)    ") { 10.times { texts.each { |t| BigDecimal(t) } } }
time("10k to_s(\"F\")             ") { 10.times { prices.each { |p| p.to_s("F") } } }
time("sqrt(2), 1000 digits      ") { BigDecimal("2").sqrt(1000) }
time("BigMath.PI(1000)          ") { BigMath.PI(1000) }
