# A thread already running when allocation tracing starts traces what it
# allocates from then on, under its own frames (#690).
require 'objspace'

queue = Queue.new
thread = Thread.new do
  queue.pop
  object = Object.new
  [File.basename(ObjectSpace.allocation_sourcefile(object)), ObjectSpace.allocation_sourceline(object)]
end
Thread.pass until thread.status == "sleep"
ObjectSpace.trace_object_allocations_start
queue << 1
p thread.value
ObjectSpace.trace_object_allocations_stop
