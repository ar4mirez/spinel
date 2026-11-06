# `Array#sample` and `#shuffle` over a seeded `Random`, by path: one element,
# up to ten drawn and placed, the sparse swap, and the partial shuffle — the
# sizes `sample_spec.rb`'s "samples evenly" draws (#697). Every draw is
# `Random#__limited__` over MT19937, which is what this measures.
#
#   target/release/spinel run bench/array_sample.rb
#   ruby --yjit bench/array_sample.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(1)} ms"
end

N = 4_000

small = Array.new(40) { |i| i }
large = Array.new(100) { |i| i }
wide = Array.new(4_000) { |i| i }
random = Random.new(42)

time("sample           ") { N.times { small.sample(random: random) } }
time("sample(3)        ") { N.times { small.sample(3, random: random) } }
time("sample(8)        ") { N.times { small.sample(8, random: random) } }
time("sample(16)       ") { N.times { small.sample(16, random: random) } }
time("sample(80) of 100") { N.times { large.sample(80, random: random) } }
time("sample(12) of 4k ") { N.times { wide.sample(12, random: random) } }
time("shuffle of 100   ") { N.times { large.shuffle(random: random) } }
