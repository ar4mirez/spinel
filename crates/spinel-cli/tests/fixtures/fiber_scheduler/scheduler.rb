# A Fiber scheduler with a `select(2)` loop, after the one CRuby's own tests
# use (`test/fiber/scheduler.rb`), that logs every hook it is asked: which,
# with what, and what the IO::Buffer it is handed says about itself.
# `fiber_scheduler.rs` runs the fixtures that use it.
class LoggingScheduler
  EAGAIN = -Errno::EAGAIN::Errno

  attr_reader :log

  def initialize
    @log = []
    @readable = {}
    @writable = {}
    @ready = []
  end

  def describe(buffer)
    flags = %i[locked? readonly? external? internal? mapped?].select { |flag| buffer.public_send(flag) }
    [buffer.class, buffer.size, *flags]
  end

  # Runs every fiber to the end: what `close` does, and so what the end of a
  # thread, or of the program, does.
  def run
    until @readable.empty? && @writable.empty? && @ready.empty?
      ready = @ready
      @ready = []
      ready.each { |fiber| fiber.resume if fiber.alive? }
      next if @readable.empty? && @writable.empty?
      readable, writable = IO.select(@readable.keys, @writable.keys)
      readable&.each { |io| @readable.delete(io).resume }
      writable&.each { |io| @writable.delete(io).resume }
    end
  end

  def close
    @log << [:close]
    run
  end

  def fiber(&block)
    @log << [:fiber]
    Fiber.new(blocking: false, &block).tap(&:resume)
  end

  def io_wait(io, events, timeout)
    @log << [:io_wait, io.class, events, timeout]
    @readable[io] = Fiber.current if events & IO::READABLE != 0
    @writable[io] = Fiber.current if events & IO::WRITABLE != 0
    Fiber.yield
    events
  end

  # At least `length` bytes into `buffer` at `offset`: a length of 0 is one
  # read of whatever is there, and `-EAGAIN` when nothing is. The read
  # itself is the buffer's, made blocking so it is not handed back here.
  def io_read(io, buffer, length, offset)
    @log << [:io_read, io.class, describe(buffer), length, offset]
    total = 0
    loop do
      result = Fiber.blocking { buffer.read(io, 0, offset) }
      @log << [:read, result]
      if result > 0
        total += result
        offset += result
        return total if total >= length
      elsif result == 0
        return total
      elsif result == EAGAIN
        return result if length == 0
        io_wait(io, IO::READABLE, nil)
      else
        return result
      end
    end
  end

  def io_write(io, buffer, length, offset)
    @log << [:io_write, io.class, describe(buffer), length, offset]
    total = 0
    loop do
      result = Fiber.blocking { buffer.write(io, 0, offset) }
      @log << [:write, result]
      if result > 0
        total += result
        offset += result
        return total if total >= length
      elsif result == 0
        return total
      elsif result == EAGAIN
        return result if length == 0
        io_wait(io, IO::WRITABLE, nil)
      else
        return result
      end
    end
  end

  # A file never waits: one read or write at the file offset `from`.
  def io_pread(io, buffer, from, length, offset)
    @log << [:io_pread, io.class, describe(buffer), from, length, offset]
    Fiber.blocking { buffer.pread(io, from, length, offset) }
  end

  def io_pwrite(io, buffer, from, length, offset)
    @log << [:io_pwrite, io.class, describe(buffer), from, length, offset]
    Fiber.blocking { buffer.pwrite(io, from, length, offset) }
  end

  def kernel_sleep(*duration)
    @log << [:kernel_sleep, *duration]
    @ready << Fiber.current
    Fiber.yield
  end

  def block(blocker, timeout = nil)
    raise NotImplementedError, "block"
  end

  def unblock(blocker, fiber)
    raise NotImplementedError, "unblock"
  end
end
