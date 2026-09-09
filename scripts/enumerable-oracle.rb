#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `core/enumerable.rb`.
#
# Enumerable has two yield conventions and the docs do not say which method uses
# which. A block passed to `map` sees the values `each` yielded, with their
# original arity; a block passed to `select` sees them packed into a single
# value. `yield 1, 2` therefore reaches a `map` block as `1` and a `select`
# block as `[1, 2]`, and there is no rule that predicts the split — `map`,
# `flat_map`, `filter_map`, `find_index`, `all?`, `any?`, `none?`, `one?`,
# `count`, `take_while`, `uniq` and `to_h` pass through, and everything else
# packs.
#
# Getting one wrong is not a crash: it is `[1, 6]` where Ruby says `[[1, 2], 6]`,
# which passes a spec whose fixture yields one value at a time and fails only on
# `EnumerableSpecs::YieldsMulti`. So the split is measured rather than reasoned
# about, and this script is the measurement.
#
#   scripts/enumerable-oracle.rb          on a real ruby, prints the table
#
# Each line prints what the block actually received, with `|*a|` so the arity is
# visible. Run it on CRuby; the comment block at the top of `core/enumerable.rb`
# is this output.

class M
  include Enumerable
  def each; yield 1, 2; yield 6; self; end
end
# What does the block actually receive?  |*a| shows the true arity.
def probe(name)
  seen = []
  begin
    yield seen
  rescue => e
    puts "#{name.ljust(18)} ERR #{e.class}"
    return
  end
  puts "#{name.ljust(18)} block saw #{seen.inspect}"
end
m = M.new
probe("map")             { |s| m.map { |*a| s << a; a } }
probe("flat_map")        { |s| m.flat_map { |*a| s << a; nil } }
probe("filter_map")      { |s| m.filter_map { |*a| s << a; nil } }
probe("select")          { |s| m.select { |*a| s << a; true } }
probe("reject")          { |s| m.reject { |*a| s << a; false } }
probe("find")            { |s| m.find { |*a| s << a; false } }
probe("find_index")      { |s| m.find_index { |*a| s << a; false } }
probe("group_by")        { |s| m.group_by { |*a| s << a; 1 } }
probe("partition")       { |s| m.partition { |*a| s << a; true } }
probe("sort_by")         { |s| m.sort_by { |*a| s << a; 1 } }
probe("min_by")          { |s| m.min_by { |*a| s << a; 1 } }
probe("max_by")          { |s| m.max_by { |*a| s << a; 1 } }
probe("each_with_object"){ |s| m.each_with_object(0) { |*a| s << a } }
probe("each_with_index") { |s| m.each_with_index { |*a| s << a } }
probe("all?")            { |s| m.all? { |*a| s << a; true } }
probe("any?")            { |s| m.any? { |*a| s << a; false } }
probe("none?")           { |s| m.none? { |*a| s << a; false } }
probe("one?")            { |s| m.one? { |*a| s << a; false } }
probe("count{}")         { |s| m.count { |*a| s << a; true } }
probe("take_while")      { |s| m.take_while { |*a| s << a; true } }
probe("drop_while")      { |s| m.drop_while { |*a| s << a; false } }
probe("inject")          { |s| m.inject(0) { |acc,*a| s << a; acc } }
probe("sum")             { |s| m.sum(0) { |*a| s << a; 0 } }
probe("each_entry")      { |s| m.each_entry { |*a| s << a } }
probe("reverse_each")    { |s| m.reverse_each { |*a| s << a } }
probe("cycle(1)")        { |s| m.cycle(1) { |*a| s << a } }
probe("chunk_while")     { |s| m.chunk_while { |*a| s << a; true }.to_a }
probe("slice_when")      { |s| m.slice_when { |*a| s << a; false }.to_a }
probe("chunk")           { |s| m.chunk { |*a| s << a; 1 }.to_a }
probe("uniq")            { |s| m.uniq { |*a| s << a; 1 } }
probe("to_h")            { |s| m.to_h { |*a| s << a; [1,2] } }
probe("tally_by?")       { |s| m.each_slice(1) { |*a| s << a } }
