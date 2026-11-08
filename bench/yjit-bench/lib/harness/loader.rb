# Spinel's stand-in for yjit-bench's `lib/harness/loader.rb`, which every
# vendored benchmark requires by this relative path. It is not the upstream
# harness: that one picks `harness/default.rb`, which reads `RubyVM`, `RbConfig`
# and RSS and writes CSV. This one keeps only the protocol the benchmarks call,
# `run_benchmark(hint) { ... }`, and runs the same way on Spinel and on CRuby so
# both sides of the table are timed by the same code.
#
#   WARMUP_ITRS  iterations run and thrown away first (default 1)
#   BENCH_ITRS   iterations timed (default 5)
#
# Each iteration prints its time; the last line is the median of the timed
# ones, which `scripts/bench.sh --yjit-bench` reads.

WARMUP_ITRS = Integer(ENV.fetch("WARMUP_ITRS", "1"))
BENCH_ITRS = Integer(ENV.fetch("BENCH_ITRS", "5"))

# yjit-bench's harnesses define this for the Ractor variants of a benchmark.
def make_shareable(obj, copy: false)
  obj
end

def run_benchmark(_num_itrs_hint, **)
  times = []
  (WARMUP_ITRS + BENCH_ITRS).times do |i|
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000.0
    puts "itr ##{i + 1}#{i < WARMUP_ITRS ? " (warmup)" : ""}: #{ms.round(1)} ms"
    times << ms if i >= WARMUP_ITRS
  end
  sorted = times.sort
  mid = sorted.size / 2
  median = sorted.size.odd? ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2.0
  puts "median_ms: #{median.round(1)}"
end
