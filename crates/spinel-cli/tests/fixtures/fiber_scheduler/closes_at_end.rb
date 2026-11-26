# A scheduler left set is closed when its thread's block returns, and the
# main thread's before the `at_exit` blocks run, which runs what each
# scheduled, as CRuby's `thread_do_start` and `rb_ec_fiber_scheduler_finalize`
# close them.
require_relative "scheduler"

at_exit { puts "at_exit, scheduler #{Fiber.scheduler.inspect}" }

value = Thread.new do
  scheduler = LoggingScheduler.new
  Fiber.set_scheduler(scheduler)
  Fiber.schedule do
    sleep 0
    puts "thread fiber done"
  end
  puts "thread block done"
  :value
end.value
puts "thread value #{value.inspect}"

Fiber.set_scheduler(LoggingScheduler.new)
Fiber.schedule do
  sleep 0
  puts "main fiber done"
end
puts "main script done"
