# Fiber (#16).
#
# Switching is the VM's: `interp.rs` keeps each fiber's value stack and frames,
# and the `__fiber_*__` primitives swap them in and out of the interpreter loop.
# Everything here is Ruby around those: the argument rules, `blocking`, the
# storage, and how a fiber describes itself. Every rule was measured against
# ruby 4.0.7.
class Fiber
  def initialize(blocking: false, storage: true, &block)
    # `Kernel.raise`: inside a Fiber, a bare `raise` is `Fiber#raise`.
    Kernel.raise ArgumentError, "tried to create Proc object without a block" if block.nil?
    @__blocking__ = blocking ? true : false
    # `storage: true` — the default — inherits a copy of the creating fiber's;
    # a Hash is used as given, once checked.
    @__storage__ = storage == true ? Fiber.current.__storage_copy__ : Fiber.__check_storage__(storage)
    @__block__ = block
    @__thread__ = __thread_current__
    __fiber_new__(self, block)
  end

  def resume(*args)
    __check_thread__
    __fiber_resume__(self, args)
  end

  def transfer(*args)
    __check_thread__
    __fiber_transfer__(self, args)
  end

  # Measured: a fiber made in one thread cannot be run from another.
  def __check_thread__
    return if @__thread__.nil? || @__thread__ == __thread_current__
    Kernel.raise FiberError, "fiber called across threads"
  end

  def self.yield(*args)
    __fiber_yield__(args)
  end

  def self.current
    __fiber_current__
  end

  def alive?
    __fiber_alive__(self)
  end

  def raise(*args)
    __fiber_raise__(self, args)
  end

  def kill
    __fiber_kill__(self)
  end

  # The root fiber is blocking; a made one is only when asked to be.
  # `Fiber.blocking?` answers 1 rather than true for a blocking fiber.
  def blocking?
    return true if __root__?
    @__blocking__
  end

  def self.blocking?
    current.blocking? ? 1 : false
  end

  # The current fiber made blocking for the block (#797), so the scheduler
  # is not asked meanwhile; a fiber already blocking is left as it is.
  def self.blocking
    fiber = current
    return yield(fiber) if fiber.blocking?
    fiber.__set_blocking__(true)
    begin
      yield fiber
    ensure
      fiber.__set_blocking__(false)
    end
  end

  def __set_blocking__(blocking)
    @__blocking__ = blocking
  end

  # --- the scheduler (#797) ---------------------------------------------------
  # One per thread, as CRuby keeps `thread->scheduler`: what `set_scheduler`
  # gave the thread. It is *current* only in a fiber that is not blocking —
  # not the root fiber, not one made `blocking: true`, not inside
  # `Fiber.blocking` — and only then do `sleep`, an IO's waits, reads and
  # writes, and `IO::Buffer#read` and `#write` hand themselves to it, as
  # `rb_fiber_scheduler_current` decides.

  def self.scheduler
    Thread.current.__scheduler__
  end

  def self.current_scheduler
    current.blocking? ? nil : Thread.current.__scheduler__
  end

  def self.set_scheduler(scheduler)
    unless scheduler.nil?
      %i[block unblock kernel_sleep io_wait].each do |hook|
        Kernel.raise ArgumentError, "Scheduler must implement ##{hook}" unless scheduler.respond_to?(hook)
      end
    end
    thread = Thread.current
    # The one there is closed first, so it runs what it has to completion
    # before another takes over; it is gone however that ends.
    previous = thread.__scheduler__
    unless previous.nil?
      begin
        __close_scheduler__(previous)
      ensure
        thread.__scheduler__ = nil
      end
    end
    thread.__scheduler__ = scheduler
  end

  # `rb_fiber_scheduler_close`: `scheduler_close` if it has one, for a
  # scheduler whose own `close` sets the scheduler to nil, else `close`.
  def self.__close_scheduler__(scheduler)
    return scheduler.__send__(:scheduler_close) if scheduler.respond_to?(:scheduler_close, true)
    return scheduler.__send__(:close) if scheduler.respond_to?(:close, true)
    nil
  end

  # The thread's scheduler, not only the current one: `Fiber.schedule` in a
  # blocking fiber still schedules.
  def self.schedule(*args, **keywords, &block)
    scheduler = Thread.current.__scheduler__
    Kernel.raise RuntimeError, "No scheduler is available!" if scheduler.nil?
    scheduler.fiber(*args, **keywords, &block)
  end

  # The end of a thread's block, and of the program before its `at_exit`
  # blocks: the scheduler left is closed, which runs what it has scheduled,
  # as CRuby's `thread_do_start` and `rb_ec_fiber_scheduler_finalize` do.
  def self.__finalize_scheduler__
    set_scheduler(nil) unless Thread.current.__scheduler__.nil?
  end

  def __root__?
    @__fiber__ == -1
  end

  # `#<Fiber:0x... path:line (status)>`, the location being where the block
  # was written; the root fiber has none.
  def inspect
    head = "#<Fiber:0x" + __address__
    unless @__block__.nil?
      location = __proc_location__(@__block__)
      head = head + " " + location[0] + ":" + location[1].to_s unless location.nil?
    end
    head + " (" + __fiber_status__(self).to_s + ")>"
  end

  def to_s
    inspect
  end

  # --- storage --------------------------------------------------------------
  # A fiber's storage is a Hash of Symbol keys. A new fiber gets a copy of its
  # creator's; only the fiber itself may read or replace its own.

  def storage
    unless equal?(Fiber.current)
      Kernel.raise ArgumentError, "Fiber storage can only be accessed from the Fiber it belongs to"
    end
    @__storage__
  end

  def storage=(hash)
    unless equal?(Fiber.current)
      Kernel.raise ArgumentError, "Fiber storage can only be accessed from the Fiber it belongs to"
    end
    @__storage__ = Fiber.__check_storage__(hash)
  end

  def __storage_copy__
    @__storage__.nil? ? nil : @__storage__.dup
  end

  def __storage__
    @__storage__
  end

  def __set_storage__(hash)
    @__storage__ = hash
  end

  def self.__check_storage__(storage)
    return nil if storage.nil?
    Kernel.raise TypeError, "storage must be a hash" unless storage.is_a?(Hash)
    Kernel.raise FrozenError, "storage must not be frozen" if storage.frozen?
    storage.each_key do |key|
      unless key.is_a?(Symbol)
        Kernel.raise TypeError, "wrong argument type #{key.class} (expected Symbol)"
      end
    end
    storage
  end

  # A storage key: a Symbol, or a String (or anything with `to_str`) turned
  # into one. Measured: `to_sym` is not called.
  def self.__key__(key)
    return key if key.is_a?(Symbol)
    return key.to_str.to_sym if key.is_a?(String) || key.respond_to?(:to_str)
    Kernel.raise TypeError, "#{key.inspect} is not a symbol nor a string"
  end

  def self.[](key)
    key = __key__(key)
    storage = current.__storage__
    storage.nil? ? nil : storage[key]
  end

  # Assigning nil deletes the key.
  def self.[]=(key, value)
    key = __key__(key)
    fiber = current
    storage = fiber.__storage__
    if value.nil?
      storage.delete(key) unless storage.nil?
      return value
    end
    if storage.nil?
      storage = {}
      fiber.__set_storage__(storage)
    end
    storage[key] = value
  end
end
