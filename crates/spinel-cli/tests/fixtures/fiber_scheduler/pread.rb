# `IO#pread` and `#pwrite`, and `IO::Buffer#pread` and `#pwrite`, in a
# non-blocking fiber: the scheduler's `io_pread` and `io_pwrite`, at the
# file offset asked for. The file is in the directory the test hands over.
require_relative "scheduler"

Warning[:experimental] = false
path = File.join(ARGV.fetch(0), "pread.txt")
File.write(path, "0123456789")
file = File.open(path, "r+")
scheduler = LoggingScheduler.new
Fiber.set_scheduler(scheduler)
Fiber.schedule do
  p file.pread(4, 3)
  p file.pwrite("AB", 1)
  buffer = IO::Buffer.new(4)
  p buffer.pread(file, 6), buffer.get_string
  p buffer.pwrite(file, 0, 2)
end
Fiber.set_scheduler(nil)
file.close
p File.read(path)
scheduler.log.each { |entry| p entry }
