# Two threads each load one of two files that require each other: each
# waits for the other's load to end, and the main thread, joining, is
# told of the deadlock rather than waiting for ever.
$LOAD_PATH.unshift File.join(__dir__, "lib")
$in_a = Queue.new
$in_b = Queue.new
a = Thread.new { require "cycle_a" }
b = Thread.new { require "cycle_b" }
a.join
b.join
puts "unreachable"
