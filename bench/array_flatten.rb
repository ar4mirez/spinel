# `Array#flatten` against nesting depth and against width, the shapes #741
# was filed for: flattening `n` levels should cost about what flattening `n`
# elements does, the way CRuby's explicit-stack `flatten()` makes it.
#
#   scripts/bench.sh array_flatten
#   target/release/spinel run bench/array_flatten.rb
#   ruby --yjit bench/array_flatten.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

[1_000, 2_000, 10_000].each do |depth|
  deep = [1]
  depth.times { deep = [deep] }
  time("deep    #{depth.to_s.rjust(6)} levels       ") { deep.flatten }
  time("deep    #{depth.to_s.rjust(6)} levels, level") { deep.flatten(depth) }
end

wide = Array.new(10_000) { [1, [2, [3]]] }
time("wide    10000 x 3 deep, x10   ") { 10.times { wide.flatten } }
