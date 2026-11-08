# What a timed wait answers, as CRuby 4.0.9: nil from `Mutex#sleep` and
# `ConditionVariable#wait` when the timeout runs out, false from a Monitor
# condition's `wait`; the seconds slept, and true, when signaled.
require "monitor"

mutex = Mutex.new
cv = Thread::ConditionVariable.new
p mutex.synchronize { mutex.sleep(0.05) }
p mutex.synchronize { cv.wait(mutex, 0.05) }
waiter = Thread.new { mutex.synchronize { cv.wait(mutex, 5) } }
Thread.pass until waiter.stop?
mutex.synchronize { cv.signal }
p waiter.value

monitor = Monitor.new
cond = monitor.new_cond
p monitor.synchronize { cond.wait(0.05) }
waiter = Thread.new { monitor.synchronize { cond.wait(5) } }
Thread.pass until waiter.stop?
monitor.synchronize { cond.signal }
p waiter.value
