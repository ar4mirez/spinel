# The IO hooks, each handed an IO::Buffer: a reader that waits for a writer
# that sleeps first, through `IO` and through `IO::Buffer` itself, all of it
# run by the scheduler's close, at `Fiber.set_scheduler(nil)`.
require_relative "scheduler"

Warning[:experimental] = false
r, w = IO.pipe
scheduler = LoggingScheduler.new
Fiber.set_scheduler(scheduler)
order = []

Fiber.schedule do
  order << [:readpartial, r.readpartial(5)]
  buffer = IO::Buffer.new(3)
  order << [:buffer_read, buffer.read(r, 3), buffer.get_string]
  order << [:read_nonblock, r.read_nonblock(10, exception: false)]
  order << [:current, Fiber.current_scheduler.equal?(scheduler), Fiber.blocking { Fiber.current_scheduler }]
end

Fiber.schedule do
  sleep 0.01
  order << [:write, w.write("hello")]
  order << [:buffer_write, IO::Buffer.for("abc").write(w)]
end

order << [:scheduled]
Fiber.set_scheduler(nil)
order.each { |entry| p entry }
scheduler.log.each { |entry| p entry }
p Fiber.scheduler
