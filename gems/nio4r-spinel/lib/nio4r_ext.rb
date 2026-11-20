# frozen_string_literal: true

# The nio4r gem's C extension (`ext/nio4r/{selector,monitor,bytebuffer}.c`
# at 2.7.5), in Ruby, over the readiness calls of `NIO::Spinel`, the gem's
# `.spinel` extension (`ext/nio4r_spinel`). Upstream's `lib/nio.rb` requires
# this file where CRuby loads `nio4r_ext.so`; the rest of `lib/` is
# upstream's, unchanged.
#
# Each method does what its C function does: the same checks in the same
# order, the same exception classes and messages, and the same answers.
# Where the C has libev watch descriptors, this keeps a table of the
# monitors on each descriptor and has the system's readiness queue watch
# the union of their interests, as libev's `anfds` do: epoll on Linux and
# kqueue on macOS and the BSDs, made in `NIO::Spinel` without waiting, and
# `poll(2)` for the `:poll` and `:select` backends. A selector waits in
# `IO.select`, on the queue's descriptor or on the monitored IOs, with its
# wakeup pipe beside them, so the wait goes through Spinel's scheduler,
# outside the Ractor lock, as libev's patched wait goes outside CRuby's GVL.
# The C's state is in instance variables spelled `@__name__`, which
# `instance_variables` leaves out as it leaves out the C's own (`rb_intern`
# names with no `@`) and a typed data struct.
#
# ponytail: `Selector.backends` lists the backends implemented here,
# `:epoll` (`:kqueue` off Linux), `:poll` and `:select`; libev's
# `:linuxaio`, `:io_uring` and `:port`, which upstream lists where libev was
# compiled with them and never picks unasked, are not. Upgrade: each as a
# queue in `NIO::Spinel`. `NIO::ENGINE` stays upstream's `"libev"`, as
# `lib/nio.rb` sets it for any native extension.

unless RUBY_ENGINE == "spinel"
  raise LoadError, "nio4r-spinel is the nio4r gem for Spinel and loads only there; " \
                   "on #{RUBY_ENGINE}, install the nio4r gem"
end

require "nio/nio4r_spinel"

module NIO
  module Spinel
    # libev's EV_READ and EV_WRITE, and what `poll_ready` adds for a
    # descriptor that is not open.
    READ = 1
    WRITE = 2
    INVALID = 4

    INTERESTS = {r: READ, w: WRITE, rw: READ | WRITE}.freeze
    SYMBOLS = INTERESTS.invert.freeze

    # libev's default backend: the first of `ev_recommended_backends`,
    # which leaves out kqueue everywhere but NetBSD and, on Apple's, poll.
    QUEUE = queue_backend.to_sym
    # In `NIO_Selector_supported_backends`' order.
    BACKENDS = (QUEUE == :epoll ? %i[epoll poll select] : %i[poll kqueue select]).freeze
    DEFAULT = QUEUE == :epoll ? :epoll : :select
    # Whether the queue marks each registration with a generation, as only
    # libev's epoll backend does: kqueue's go with their descriptor.
    GENERATIONS = QUEUE == :epoll
    INT = (-2**31...2**31)

    class << self
      # `rb_raise(rb_eTypeError, "...")`'s name for a value in a conversion
      # message: `nil`, `true` and `false` by name, anything else by class.
      def type_name(value)
        case value
        when nil, true, false then value.inspect
        else value.class.to_s
        end
      end

      # `SYM2ID`, as nio4r calls it on an interest.
      def symbol(value)
        return value if Symbol === value
        raise TypeError, "wrong argument type #{value.class} (expected Symbol)"
      end

      # `rb_convert_type(io, T_FILE, "IO", "to_io")`.
      def to_io(value)
        return value if IO === value
        unless value.respond_to?(:to_io, true)
          raise TypeError, "no implicit conversion of #{type_name(value)} into IO"
        end
        io = value.__send__(:to_io)
        return io if IO === io
        raise TypeError, "can't convert #{value.class} to IO (#{value.class}#to_io gives #{io.class})"
      end

      # `NUM2DBL`.
      def num2dbl(value)
        case value
        when Float then value
        when Integer, Rational then value.to_f
        when String then raise TypeError, "no implicit conversion to float from string"
        when nil, true, false then raise TypeError, "no implicit conversion to float from #{value.inspect}"
        when Numeric then value.to_f
        else raise TypeError, "can't convert #{value.class} into Float"
        end
      end

      # `NUM2INT`: an Integer, or a Float truncated, in a C int.
      def num2int(value)
        int = case value
              when Integer then value
              when nil then raise TypeError, "no implicit conversion from nil to integer"
              when Float then value.to_i
              else
                unless value.respond_to?(:to_int, true)
                  raise TypeError, "no implicit conversion of #{type_name(value)} into Integer"
                end
                value.__send__(:to_int)
              end
        unless Integer === int
          raise TypeError, "can't convert #{value.class} to Integer (#{value.class}#to_int gives #{int.class})"
        end
        unless INT.cover?(int)
          raise RangeError, "integer #{int} too #{int.negative? ? "small" : "big"} to convert to 'int'"
        end
        int
      end

      # `rb_sys_fail(name)` for a call that answered `-errno`.
      def check(status, name)
        return status unless Integer === status && status.negative?
        raise SystemCallError.new(name, -status)
      end

      # What a selector's finalizer frees: its wakeup pipe and its queue.
      def selector_finalizer(state)
        proc { shutdown(state) }
      end

      # NIO_Selector_shutdown: `state` is `[queue, reader, writer]`, the
      # queue nil for the poll backends, and emptied once freed.
      def shutdown(state)
        return if state.empty?
        queue, reader, writer = state
        reader.close unless reader.closed?
        writer.close unless writer.closed?
        close(queue) if queue
        state.clear
      end
    end
  end

  # selector.c: a set of monitors and the readiness queue they are in.
  class Selector
    def self.backends
      Spinel::BACKENDS.dup
    end

    def initialize(backend = nil)
      unless backend.nil?
        unless Spinel::BACKENDS.include?(backend)
          raise ArgumentError, "unsupported backend: #{backend.inspect}"
        end
      end
      backend ||= Spinel::DEFAULT
      reader, writer = IO.pipe
      queue = Spinel.check(Spinel.queue_new, "epoll_create") if backend == Spinel::QUEUE
      @__state__ = [queue, reader, writer]
      ObjectSpace.define_finalizer(self, Spinel.selector_finalizer(@__state__))
      @__queue_io__ = IO.for_fd(queue, autoclose: false) if queue
      @__backend__ = backend
      @__closed__ = false
      @__wakeup_fired__ = false
      # The descriptors watched: each one's monitors, and the interests the
      # queue has for it.
      @__fds__ = {}
      @__watched__ = {}
      # The descriptors epoll refuses, such as regular files', which libev
      # keeps in `epoll_eperms` and reports ready on every run.
      @__eperms__ = {}
      # Each descriptor's generation, which the queue holds beside it, as
      # libev's `egen`: a registration with another is a stray.
      @__gens__ = Hash.new(0)
      @__selectables__ = {}
      @__lock__ = Mutex.new
      @__lock_holder__ = nil
      nil
    end

    def backend
      raise IOError, "selector is closed" if @__closed__
      @__backend__
    end

    def register(io, interests)
      __synchronize__ do
        raise IOError, "selector is closed" if @__closed__
        if @__selectables__.key?(io)
          raise ArgumentError, "this IO is already registered with selector"
        end
        monitor = Monitor.new(io, interests, self)
        @__selectables__[monitor.io] = monitor
        monitor
      end
    end

    def deregister(io)
      __synchronize__ do
        monitor = @__selectables__.delete(io)
        monitor&.close(false)
        monitor
      end
    end

    def registered?(io)
      @__selectables__.key?(io)
    end

    def select(timeout = nil)
      if !timeout.nil? && Spinel.num2dbl(timeout) < 0
        raise ArgumentError, "time interval must be positive"
      end
      __synchronize__ do
        raise IOError, "selector is closed" if @__closed__
        ready = block_given? ? nil : []
        count = __run__(timeout, ready) { |monitor| yield monitor }
        if count.nil?
          nil
        else
          block_given? ? count : ready
        end
      end
    end

    def wakeup
      raise IOError, "selector is closed" if @__closed__
      @__wakeup_fired__ = true
      @__state__[2].write_nonblock("\0", exception: false)
      nil
    end

    def close
      __synchronize__ do
        Spinel.shutdown(@__state__)
        @__closed__ = true
        nil
      end
    end

    def closed?
      __synchronize__ { @__closed__ }
    end

    def empty?
      @__selectables__.empty?
    end

    # What a monitor asks of its selector's loop: `monitor` on `fd` with
    # the interests `mask` (0 for none), as `ev_io_start` and `ev_io_stop`.
    def __watch__(monitor, fd, mask)
      return if @__closed__
      monitors = @__fds__.fetch(fd, {}).merge(monitor => mask)
      monitors.delete(monitor) if mask.zero?
      queue = @__state__[0]
      if queue
        old = @__watched__.fetch(fd, 0)
        new = monitors.each_value.inject(0, :|)
        status = old == new ? 0 : Spinel.queue_set(queue, fd, old, new, @__gens__[fd])
        if new.zero?
          # A descriptor closed while watched has left the queue already, and
          # epoll refuses to remove it; libev lets that pass too.
          __unwatch__(fd)
          @__eperms__.delete(fd)
        elsif status == -Errno::EPERM::Errno
          __unwatch__(fd)
          @__eperms__[fd] = true
        else
          Spinel.check(status, "epoll_ctl")
          @__watched__[fd] = new
          @__eperms__.delete(fd)
        end
      end
      if monitors.empty?
        @__fds__.delete(fd)
      else
        @__fds__[fd] = monitors
      end
    end

    private

    # `fd` out of what the queue watches, its generation moved on.
    def __unwatch__(fd)
      return unless @__watched__.delete(fd) && Spinel::GENERATIONS
      @__gens__[fd] = (@__gens__[fd] + 1) & 0x7FFF_FFFF
    end

    # NIO_Selector_synchronize: a lock the thread holding it takes again.
    def __synchronize__
      if @__lock_holder__ == Thread.current
        yield
      else
        @__lock__.lock
        begin
          @__lock_holder__ = Thread.current
          yield
        ensure
          @__lock_holder__ = nil
          @__lock__.unlock
        end
      end
    end

    # NIO_Selector_run, as `ev_run(EVRUN_ONCE)` with a timer: wait until a
    # descriptor is ready, the wakeup pipe is written or `timeout` passes,
    # then hand each ready monitor to the block, or push it on `ready`. The
    # count of monitors, or nil when none was ready and no wakeup came.
    def __run__(timeout, ready, &block)
      @__wakeup_fired__ = false
      timeout = timeout.nil? ? nil : Spinel.num2dbl(timeout)
      deadline = timeout && Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
      count = 0
      loop do
        count = __collect__(ready, &block)
        break if count.positive? || timeout&.zero?
        remaining = deadline && deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
        break if remaining && remaining <= 0
        readers, writers = __waiting_on__
        woken = IO.select(readers, writers, nil, remaining)
        next unless woken && woken[0].include?(@__state__[1])
        # The wakeup pipe, drained, which ends the run like any event.
        nil while (bytes = @__state__[1].read_nonblock(128, exception: false)).is_a?(String) && !bytes.empty?
        count = __collect__(ready, &block)
        break
      end
      return count if count.positive? || @__wakeup_fired__
      nil
    ensure
      @__wakeup_fired__ = false
    end

    # What a wait waits on: the queue's descriptor, or every open IO being
    # watched; and the wakeup pipe.
    def __waiting_on__
      reader = @__state__[1]
      return [[@__queue_io__, reader], nil] if @__state__[0]
      readers = [reader]
      writers = []
      @__fds__.each_value do |monitors|
        monitors.each do |monitor, mask|
          io = Spinel.to_io(monitor.io)
          next if io.closed?
          readers << io if mask & Spinel::READ != 0
          writers << io if mask & Spinel::WRITE != 0
        end
      end
      [readers.uniq, writers.uniq]
    end

    # The ready descriptors now, without waiting: each event goes to the
    # monitors on its descriptor that asked for it, as libev's `fd_event`.
    def __collect__(ready)
      queue = @__state__[0]
      if queue
        events = Spinel.check(Spinel.queue_wait(queue), "epoll_wait").unpack("l<*").each_slice(Spinel::GENERATIONS ? 3 : 2).to_a
        events = __drop_strays__(queue, events)
        @__eperms__.each_key { |fd| events << [fd, Spinel::READ | Spinel::WRITE] }
      else
        pairs = @__fds__.map { |fd, monitors| [fd, monitors.each_value.inject(0, :|)] }
        events = pairs.empty? ? "".b : Spinel.check(Spinel.poll_ready(pairs.flatten.pack("l<*")), "poll")
        events = events.unpack("l<*").each_slice(2).to_a
        if @__backend__ == :select && events.any? { |_, got| got & Spinel::INVALID != 0 }
          # select(2) fails with EBADF on a descriptor that is not open, and
          # libev's select backend then kills those (`fd_ebadf`) and reports
          # nothing else that run; poll(2) reports POLLNVAL beside the rest.
          events = events.select { |_, got| got & Spinel::INVALID != 0 }
        end
      end
      count = 0
      events.each do |fd, got|
        monitors = @__fds__[fd] or next
        killed = got & Spinel::INVALID != 0
        if killed
          # fd_kill: a descriptor that is not open stops its watchers, each
          # handed one last event of everything, whatever its interests.
          got = Spinel::READ | Spinel::WRITE
          monitors.each_key { |monitor| monitor.__stop__ }
          @__fds__.delete(fd)
        end
        monitors.to_a.each do |monitor, mask|
          revents = killed ? got : got & mask
          next if revents.zero?
          monitor.__readiness__ = revents
          count += 1
          if ready
            ready << monitor
          else
            yield monitor
          end
        end
      end
      count
    end

    # epoll_poll's check of what an event got against what the queue was
    # asked for: a registration nobody wants, such as one a descriptor
    # closed while its description stayed open elsewhere left behind, is
    # changed to what is wanted; one of another generation, or one the
    # queue refuses to change, is dropped and the queue made again from
    # what is watched, as libev's `postfork |= 2` makes it. The events
    # left, as `(fd, mask)` pairs.
    def __drop_strays__(queue, events)
      rebuild = false
      kept = events.filter_map do |fd, got, gen|
        if Spinel::GENERATIONS && gen != @__gens__[fd]
          rebuild = true
          next
        end
        want = @__watched__.fetch(fd, 0)
        unless got & ~want == 0
          status = Spinel.queue_set(queue, fd, Spinel::READ | Spinel::WRITE, want, @__gens__[fd])
          unless status.zero?
            rebuild = true
            next
          end
        end
        [fd, got]
      end
      __rebuild_queue__ if rebuild
      kept
    end

    # A new queue in the old one's place, watching what `@__watched__`
    # holds; the old one closed.
    def __rebuild_queue__
      queue = Spinel.check(Spinel.queue_new, "epoll_create")
      old = @__state__[0]
      @__state__[0] = queue
      @__queue_io__ = IO.for_fd(queue, autoclose: false)
      Spinel.close(old)
      @__watched__.to_a.each do |fd, mask|
        status = Spinel.queue_set(queue, fd, 0, mask, @__gens__[fd])
        next if status.zero?
        __unwatch__(fd)
        @__eperms__[fd] = true if status == -Errno::EPERM::Errno
      end
    end
  end

  # monitor.c: an IO, the interests a selector watches it for, and what it
  # was ready for last.
  class Monitor
    def initialize(io, interests, selector)
      mask = Spinel::INTERESTS[Spinel.symbol(interests)]
      unless mask
        raise ArgumentError, "invalid event type #{interests.inspect} (must be :r, :w, or :rw)"
      end
      @__fd__ = Spinel.to_io(io).fileno
      @__interests_mask__ = mask
      @__revents__ = 0
      @__io__ = io
      @__interests__ = interests
      @__selector__ = selector
      @__loop__ = selector
      selector.__watch__(self, @__fd__, mask)
      nil
    end

    def close(*args)
      if args.size > 1
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)"
      end
      deregister = args.fetch(0, nil)
      selector = @__selector__
      unless selector.nil?
        @__loop__&.__watch__(self, @__fd__, 0) unless @__interests_mask__.zero?
        @__loop__ = nil
        @__selector__ = nil
        selector.deregister(@__io__) if deregister == true || deregister.nil?
      end
      nil
    end

    def closed?
      @__loop__.nil?
    end

    def io
      @__io__
    end

    def interests
      @__interests__
    end

    def interests=(interests)
      __update_interests__(interests.nil? ? 0 : __interest__(interests))
      @__interests__
    end

    def add_interest(interest)
      __update_interests__(@__interests_mask__ | __interest__(interest))
      @__interests__
    end

    def remove_interest(interest)
      __update_interests__(@__interests_mask__ & ~__interest__(interest))
      @__interests__
    end

    def selector
      @__selector__
    end

    def value
      @__value__
    end

    def value=(value)
      @__value__ = value
    end

    def readiness
      Spinel::SYMBOLS[@__revents__ & (Spinel::READ | Spinel::WRITE)]
    end

    def readable?
      @__revents__ & Spinel::READ != 0
    end

    def writable?
      @__revents__ & Spinel::WRITE != 0
    end
    alias_method :writeable?, :writable?

    # The selector's: what an event found this monitor ready for.
    def __readiness__=(revents)
      @__revents__ = revents
    end

    # The selector's, for a descriptor that is not open: its watcher
    # stopped, as libev's `fd_kill` stops it, the interests kept.
    def __stop__
      @__interests_mask__ = 0
    end

    private

    # NIO_Monitor_symbol2interest.
    def __interest__(interest)
      Spinel::INTERESTS[Spinel.symbol(interest)] or
        raise ArgumentError, "invalid interest type #{interest.inspect} (must be :r, :w, or :rw)"
    end

    # NIO_Monitor_update_interests.
    def __update_interests__(mask)
      raise EOFError, "monitor is closed" if closed?
      unless @__interests_mask__ == mask
        @__loop__.__watch__(self, @__fd__, mask)
        @__interests_mask__ = mask
      end
      @__interests__ = Spinel::SYMBOLS[mask]
    end
  end

  # bytebuffer.c: a fixed buffer of bytes with Java NIO's position, limit
  # and mark.
  class ByteBuffer
    include Enumerable

    class OverflowError < IOError; end
    class UnderflowError < IOError; end
    class MarkUnsetError < IOError; end

    # A byte as the C reads it out of its `char *`: negative past 127 where
    # a `char` is signed.
    SIGNED = Spinel.char_signed?

    def initialize(capacity)
      @__capacity__ = Spinel.num2int(capacity)
      if @__capacity__.negative?
        raise NoMemoryError, "failed to allocate memory"
      end
      @__buffer__ = "\0".b * @__capacity__
      clear
      self
    end

    def clear
      @__buffer__ = "\0".b * @__capacity__
      @__position__ = 0
      @__limit__ = @__capacity__
      @__mark__ = -1
      self
    end

    def position
      @__position__
    end

    def position=(new_position)
      pos = Spinel.num2int(new_position)
      raise ArgumentError, "negative position given" if pos.negative?
      raise ArgumentError, "specified position exceeds limit" if pos > @__limit__
      @__position__ = pos
      @__mark__ = -1 if @__mark__ > @__position__
      new_position
    end

    def limit
      @__limit__
    end

    def limit=(new_limit)
      lim = Spinel.num2int(new_limit)
      raise ArgumentError, "negative limit given" if lim.negative?
      raise ArgumentError, "specified limit exceeds capacity" if lim > @__capacity__
      @__limit__ = lim
      @__position__ = lim if @__position__ > lim
      @__mark__ = -1 if @__mark__ > lim
      new_limit
    end

    def capacity
      @__capacity__
    end
    alias_method :size, :capacity

    def remaining
      @__limit__ - @__position__
    end

    def full?
      @__position__ == @__limit__
    end

    def get(length = nil)
      len = length.nil? ? @__limit__ - @__position__ : Spinel.num2int(length)
      raise ArgumentError, "negative length given" if len.negative?
      raise UnderflowError, "not enough data in buffer" if len > @__limit__ - @__position__
      result = @__buffer__.byteslice(@__position__, len)
      @__position__ += len
      result
    end

    def [](index)
      i = Spinel.num2int(index)
      raise ArgumentError, "negative index given" if i.negative?
      raise ArgumentError, "specified index exceeds limit" if i >= @__limit__
      __byte__(i)
    end

    def <<(string)
      string = __string_value__(string)
      length = string.bytesize
      raise OverflowError, "buffer is full" if length > @__limit__ - @__position__
      __splice__(@__position__, string.b)
      @__position__ += length
      self
    end

    def read_from(io)
      fd = __nonblock__(io)
      nbytes = @__limit__ - @__position__
      raise OverflowError, "buffer is full" if nbytes.zero?
      bytes = Spinel.read(fd, nbytes)
      if Integer === bytes
        return 0 if bytes == -Errno::EAGAIN::Errno
        Spinel.check(bytes, "write")
      end
      __splice__(@__position__, bytes)
      @__position__ += bytes.bytesize
      bytes.bytesize
    end

    def write_to(io)
      fd = __nonblock__(io)
      nbytes = @__limit__ - @__position__
      raise UnderflowError, "no data remaining in buffer" if nbytes.zero?
      written = Spinel.write(fd, @__buffer__.byteslice(@__position__, nbytes))
      return 0 if written == -Errno::EAGAIN::Errno
      Spinel.check(written, "write")
      @__position__ += written
      written
    end

    def flip
      @__limit__ = @__position__
      @__position__ = 0
      @__mark__ = -1
      self
    end

    def rewind
      @__position__ = 0
      @__mark__ = -1
      self
    end

    def mark
      @__mark__ = @__position__
      self
    end

    def reset
      raise MarkUnsetError, "mark has not been set" if @__mark__.negative?
      @__position__ = @__mark__
      self
    end

    def compact
      rest = @__buffer__.byteslice(@__position__, @__limit__ - @__position__)
      __splice__(0, rest)
      @__position__ = @__limit__ - @__position__
      @__limit__ = @__capacity__
      self
    end

    def each
      raise ArgumentError, "no block given" unless block_given?
      i = 0
      while i < @__limit__
        yield __byte__(i)
        i += 1
      end
      self
    end

    def inspect
      address = Kernel.instance_method(:to_s).bind_call(self)[/0x\h+/]
      "#<#{self.class}:#{address} @position=#{@__position__} @limit=#{@__limit__} @capacity=#{@__capacity__}>"
    end

    private

    # `memcpy` into the buffer at `offset`: both Strings binary, so `[]=`
    # counts bytes.
    def __splice__(offset, bytes)
      @__buffer__[offset, bytes.bytesize] = bytes
    end

    def __byte__(index)
      byte = @__buffer__.getbyte(index)
      SIGNED && byte > 127 ? byte - 256 : byte
    end

    # `StringValue`.
    def __string_value__(value)
      return value if String === value
      return value.to_str if value.respond_to?(:to_str, true)
      raise TypeError, "no implicit conversion of #{Spinel.type_name(value)} into String"
    end

    # `rb_convert_type` to an IO, then `rb_io_set_nonblock`: its descriptor.
    def __nonblock__(io)
      fd = Spinel.to_io(io).fileno
      Spinel.check(Spinel.set_nonblock(fd), "fcntl")
      fd
    end
  end
end
