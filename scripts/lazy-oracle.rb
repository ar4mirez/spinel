#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `Enumerator::Lazy` in `core/enumerator.rb`.
#
# `Enumerable` has two yield conventions and `scripts/enumerable-oracle.rb`
# measured which method uses which. `Enumerator::Lazy` reimplements the same
# method names and *does not* inherit that table: two of them disagree with
# their eager twin, and neither disagreement is guessable.
#
#   `drop_while`  eager packs the yielded values into one array; lazy passes
#                 them through, so `yield 1, 2` reaches the eager block as
#                 `[1, 2]` and the lazy block as `1, 2`.
#   `uniq`        the other way round: eager passes through, lazy packs.
#
# Getting one wrong is silent. Every spec whose fixture yields a single value
# per iteration passes either way, and only `EnumerableSpecs::YieldsMulti`
# separates them — so the split is measured rather than reasoned about, and
# this script is the measurement.
#
#   scripts/lazy-oracle.rb        on a real ruby, prints the tables below
#
# The three tables are, in order: the yield convention per method, the size a
# lazy chain reports without iterating, and the `inspect` shape.

multi = Object.new
def multi.each
  yield 1, 2
  yield 3, 4
  self
end
multi.extend(Enumerable)

PATTERNED = %w[grep grep_v].freeze

puts "# yield convention — source yields `1, 2`; `[1, 2]` means the block took two"
printf("# %-12s %-26s %-26s\n", "method", "eager block saw", "lazy block saw")
%w[map select filter reject take_while drop_while flat_map filter_map uniq grep grep_v].each do |name|
  saw = [[], []]
  [multi, multi.lazy].each_with_index do |source, i|
    args = PATTERNED.include?(name) ? [name, Object] : [name]
    result = source.send(*args) { |*a| saw[i] << a; true }
    result.force if result.is_a?(Enumerator::Lazy)
  rescue StandardError => e
    saw[i] = "raised #{e.class}"
  end
  flag = saw[0] == saw[1] ? "" : "  <- DIFFERENT"
  printf("  %-12s %-26s %-26s%s\n", name, saw[0].inspect, saw[1].inspect, flag)
end

puts
puts "# size without iterating — nil means \"not known\""
source = (1..10).lazy
{
  "source" => source, "map" => source.map { |x| x }, "select" => source.select { |x| x },
  "reject" => source.reject { |x| x }, "take(2)" => source.take(2), "take(20)" => source.take(20),
  "drop(3)" => source.drop(3), "flat_map" => source.flat_map { |x| x }, "uniq" => source.uniq,
  "with_index" => source.with_index, "zip" => source.zip([1]), "compact" => source.compact,
  "take_while" => source.take_while { |x| x }, "filter_map" => source.filter_map { |x| x },
  "grep" => source.grep(Integer)
}.each { |name, chain| printf("  %-12s %p\n", name, chain.size) }

puts
puts "# inspect — the chain prints itself, innermost source first"
[(1..3).lazy, (1..3).lazy.map { |x| x }, (1..3).lazy.select { |x| x }.take(2),
 (1..3).lazy.with_index(3), [1, 2].each + [3].each, Enumerator.product([1, 2], [3, 4])]
  .each { |chain| puts "  #{chain.inspect}" }
