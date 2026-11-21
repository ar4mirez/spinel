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
      # The descriptors watched: each one's monitors, and, on kqueue, the
      # interests the queue has for it.
      @__fds__ = {}
      @__watched__ = {}
      # epoll's: what the kernel was last given for each descriptor (libev's
      # `emask`, EPERM_MASK for one it refused); and the descriptors it
      # refuses, such as regular files', in libev's `epoll_eperms` order,
      # reported ready on every run.
      @__emasks__ = {}
      @__eperms__ = []
      # libev's `fd_reify`, on the poll backends and epoll: the descriptors
      # changed since the last run, in `fdchanges` order; the interests the
      # backend was last given for each (`anfd->events`); and, on poll, the
      # descriptors in `polls` slot order.
      @__changes__ = []
      @__reified__ = {}
      # The descriptors a watcher started on since the last run, which
      # `fd_reify` gives the backend again (`EV__IOFDSET`).
      @__iofdset__ = {}
      # Whether epoll's kernel state is to be made again on the next run.
      @__postfork__ = false
      @__slots__ = []
      # Monitors whose watcher stopped while a run handed events over, which
      # `ev_io_stop`'s `clear_pending` takes off the pending queue.
      @__stopped__ = nil
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
    # Each call stops the monitor's watcher, if it was started, and starts it
    # again with `mask`, so it goes to the head of the descriptor's watcher
    # list (`wlist_add`): `@__fds__[fd]` keeps them oldest first. The backend
    # is told on the next run (`fd_change`), except kqueue, told at once.
    def __watch__(monitor, fd, mask)
      return if @__closed__
      @__stopped__[monitor] = true if @__stopped__
      monitors = @__fds__.fetch(fd, {}).dup
      monitors.delete(monitor)
      monitors[monitor] = mask unless mask.zero?
      queue = @__state__[0]
      if queue && !Spinel::GENERATIONS
        old = @__watched__.fetch(fd, 0)
        new = monitors.each_value.inject(0, :|)
        unless old == new
          Spinel.check(Spinel.queue_set(queue, fd, old, new, 0), "kevent")
          new.zero? ? @__watched__.delete(fd) : @__watched__[fd] = new
        end
      else
        @__changes__ << fd unless @__changes__.include?(fd)
        @__iofdset__[fd] = true unless mask.zero?
      end
      if monitors.empty?
        @__fds__.delete(fd)
      else
        @__fds__[fd] = monitors
      end
    end

    private

    # What epoll's `emask` holds for a descriptor it refused (EV_EMASK_EPERM).
    EPERM_MASK = -1

    # The interests the monitors on `fd` want together (`anfd->events`).
    def __wanted__(fd)
      @__fds__.fetch(fd, {}).each_value.inject(0, :|)
    end

    # fd_reify: each changed descriptor whose interests differ from what the
    # backend was last given, or with a watcher started on it, goes through
    # the backend's modify.
    def __reify__
      __rearm__ if @__postfork__
      queue = @__state__[0]
      @__changes__.each do |fd|
        old = @__reified__.fetch(fd, 0)
        new = __wanted__(fd)
        next if old == new && !@__iofdset__.delete(fd)
        new.zero? ? @__reified__.delete(fd) : @__reified__[fd] = new
        queue ? __epoll_modify__(queue, fd, old, new) : __poll_modify__(fd, old, new)
      end
      @__changes__.clear
      @__iofdset__.clear
    end

    # poll_modify: a new descriptor takes the next slot of `polls`, and a
    # removed one's slot goes to the last.
    def __poll_modify__(fd, old, new)
      return if old == new
      if new.zero?
        idx = @__slots__.index(fd)
        last = @__slots__.pop
        @__slots__[idx] = last if idx < @__slots__.size
      elsif old.zero?
        @__slots__ << fd
      end
    end

    # epoll_modify: no interests leaves the kernel's registration in place,
    # to be dropped when it reports something unwanted; otherwise the
    # registration is added, or changed when the kernel holds another mask,
    # under the descriptor's next generation. One epoll refuses joins
    # `epoll_eperms`, unless its mask said so already. Any other refusal
    # leaves the descriptor unwatched, its generation back, as libev's
    # `dec_egen` does past its assertion.
    def __epoll_modify__(queue, fd, old, new)
      return if new.zero?
      oldmask = @__emasks__.fetch(fd, 0)
      @__emasks__[fd] = new
      gen = @__gens__[fd] = (@__gens__[fd] + 1) & 0x7FFF_FFFF
      status = Spinel.queue_set(queue, fd, !old.zero? && oldmask != new ? oldmask : 0, new, gen)
      return if status.zero?
      if status == -Errno::EPERM::Errno
        @__emasks__[fd] = EPERM_MASK
        @__eperms__ << fd unless oldmask == EPERM_MASK
      else
        @__gens__[fd] = (gen - 1) & 0x7FFF_FFFF
      end
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
      if @__state__[0]
        __reify__ if Spinel::GENERATIONS
        return [[@__queue_io__, reader], nil]
      end
      __reify__ if @__state__[0].nil?
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

    # The ready descriptors now, without waiting, handed over in libev's
    # order: the backend feeds each event (`fd_event`) to the watchers on its
    # descriptor that asked for it, newest started first, onto the pending
    # queue, and `ev_invoke_pending` invokes that queue last fed first. The
    # backends feed descriptors in this order: epoll and kqueue as the kernel
    # returns them, then epoll's `epoll_eperms` from the last; poll by `polls`
    # slot; select from the highest descriptor down, or, when select(2) fails
    # with EBADF, only the killed ones from the lowest up (`fd_ebadf`).
    def __collect__(ready, &block)
      queue = @__state__[0]
      if queue
        __reify__ if Spinel::GENERATIONS
        events = Spinel.check(Spinel.queue_wait(queue), "epoll_wait").unpack("l<*").each_slice(Spinel::GENERATIONS ? 3 : 2).to_a
        events = __drop_strays__(queue, events)
        __eperm_events__(events) if Spinel::GENERATIONS
      else
        __reify__
        pairs = @__slots__.map { |fd| [fd, @__reified__[fd]] }
        events = pairs.empty? ? "".b : Spinel.check(Spinel.poll_ready(pairs.flatten.pack("l<*")), "poll")
        events = events.unpack("l<*").each_slice(2).to_a
        if @__backend__ == :select
          if events.any? { |_, got| got & Spinel::INVALID != 0 }
            # select(2) fails with EBADF on a descriptor that is not open, and
            # libev's select backend then kills those (`fd_ebadf`) and reports
            # nothing else that run; poll(2) reports POLLNVAL beside the rest.
            events = events.select { |_, got| got & Spinel::INVALID != 0 }.sort_by(&:first)
          else
            events = events.sort_by { |fd, _| -fd }
          end
        end
      end
      pending = {}
      events.each do |fd, got|
        monitors = @__fds__[fd] or next
        killed = got & Spinel::INVALID != 0
        if killed
          # fd_kill: a descriptor that is not open stops its watchers, each
          # handed one last event of everything, whatever its interests.
          got = Spinel::READ | Spinel::WRITE
          monitors.each_key { |monitor| monitor.__stop__ }
          @__fds__.delete(fd)
          @__changes__ << fd unless queue || @__changes__.include?(fd)
        end
        monitors.to_a.reverse_each do |monitor, mask|
          revents = killed ? got : got & mask
          # ev_feed_event: one already pending keeps its place.
          pending[monitor] = pending.fetch(monitor, 0) | revents unless revents.zero?
        end
      end
      __invoke__(pending.to_a.reverse, ready, &block)
    end

    # ev_invoke_pending: each monitor its event, unless its watcher stopped
    # since it was fed; the count of monitors.
    def __invoke__(pending, ready)
      count = 0
      outer = @__stopped__
      @__stopped__ = {}
      pending.each do |monitor, revents|
        next if @__stopped__.key?(monitor)
        monitor.__readiness__ = revents
        count += 1
        if ready
          ready << monitor
        else
          yield monitor
        end
      end
      count
    ensure
      outer&.merge!(@__stopped__)
      @__stopped__ = outer
    end

    # epoll_poll's walk of `epoll_eperms` from the last: one still marked and
    # wanted is ready for what is wanted; any other leaves, the last taking
    # its place.
    def __eperm_events__(events)
      i = @__eperms__.size
      while (i -= 1) >= 0
        fd = @__eperms__[i]
        wanted = __wanted__(fd) & (Spinel::READ | Spinel::WRITE)
        if @__emasks__[fd] == EPERM_MASK && !wanted.zero?
          events << [fd, wanted]
        else
          last = @__eperms__.pop
          @__eperms__[i] = last if i < @__eperms__.size
          @__emasks__.delete(fd) if @__emasks__[fd] == EPERM_MASK
        end
      end
    end

    # epoll_poll's and kqueue_poll's check of what an event got against
    # what the queue was asked for: a registration nobody wants, such as one
    # epoll keeps after its interests went, or one a descriptor closed while
    # its description stayed open elsewhere left behind, is changed to what
    # is wanted, or removed; one of another generation, or one the queue
    # refuses to change, is dropped and the queue made again, as libev's
    # `postfork |= 2` makes it. The events left, as `(fd, mask)` pairs.
    def __drop_strays__(queue, events)
      rebuild = false
      kept = events.filter_map do |fd, got, gen|
        if Spinel::GENERATIONS && gen != @__gens__[fd]
          rebuild = true
          next
        end
        want = (Spinel::GENERATIONS ? @__reified__ : @__watched__).fetch(fd, 0)
        unless got & ~want == 0
          if Spinel::GENERATIONS
            want.zero? ? @__emasks__.delete(fd) : @__emasks__[fd] = want
          end
          status = Spinel.queue_set(queue, fd, Spinel::READ | Spinel::WRITE, want, @__gens__[fd])
          unless status.zero?
            rebuild = true
            next
          end
        end
        [fd, got]
      end
      if rebuild
        # epoll_fork runs at the start of the next run, after this one's
        # monitors are handed over.
        Spinel::GENERATIONS ? @__postfork__ = true : __rebuild_queue__
      end
      kept
    end

    # A new queue in the old one's place, the old one closed.
    def __new_queue__
      queue = Spinel.check(Spinel.queue_new, "epoll_create")
      old = @__state__[0]
      @__state__[0] = queue
      @__queue_io__ = IO.for_fd(queue, autoclose: false)
      Spinel.close(old)
      queue
    end

    # epoll_fork's `fd_rearm_all`: every descriptor with interests, from the
    # lowest up, is changed as if new, for `fd_reify` to add to the new queue.
    def __rearm__
      @__postfork__ = false
      __new_queue__
      @__reified__.keys.sort.each do |fd|
        @__reified__.delete(fd)
        @__emasks__.delete(fd)
        @__changes__ << fd unless @__changes__.include?(fd)
      end
    end

    # kqueue's: watching what `@__watched__` holds.
    def __rebuild_queue__
      queue = __new_queue__
      @__watched__.to_a.each do |fd, mask|
        status = Spinel.queue_set(queue, fd, 0, mask, 0)
        @__watched__.delete(fd) unless status.zero?
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
