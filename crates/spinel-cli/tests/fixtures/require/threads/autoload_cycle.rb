# Two threads each reference a constant whose autoloaded file references
# the other's: each waits for the other's autoload to end, and the main
# thread, joining, is told of the deadlock rather than waiting for ever.
$in_x = Queue.new
$in_y = Queue.new
autoload :CycleX, File.join(__dir__, "lib/cycle_x.rb")
autoload :CycleY, File.join(__dir__, "lib/cycle_y.rb")
x = Thread.new { CycleX }
y = Thread.new { CycleY }
x.join
y.join
puts "unreachable"
