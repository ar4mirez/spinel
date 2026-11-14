# `monitor` (#46): CRuby's `ext/monitor`. Its C half, `monitor.c`, is the
# `Monitor` class first below, in Ruby over `Thread::Mutex`; its Ruby half,
# `MonitorMixin` and the rest of `Monitor`, is upstream's
# `ext/monitor/lib/monitor.rb` (Ruby 4.0), after it, unchanged but for its
# `require 'monitor.so'`, which is the half above, and its magic comment.
#
# A monitor is a reentrant mutex: its owner is a fiber, as in CRuby, and a
# count of how many times that fiber has entered it. Blocking is the mutex's,
# so a thread waiting to enter one sleeps in the scheduler rather than spins.
class Monitor
  # CRuby's is a data object, so `Marshal.dump` refuses one.
  Marshal.__opaque_class__(self)

  def initialize
    @__mutex__ = Thread::Mutex.new
    @__owner__ = nil
    @__count__ = 0
  end

  # A copy is a new monitor, as CRuby's allocator makes one for `dup`.
  def initialize_copy(_other)
    @__mutex__ = Thread::Mutex.new
    @__owner__ = nil
    @__count__ = 0
  end

  def try_enter
    unless __owned__?
      return false unless @__mutex__.try_lock
      @__owner__ = Fiber.current
      @__count__ = 0
    end
    @__count__ += 1
    true
  end

  def enter
    unless __owned__?
      @__mutex__.lock
      @__owner__ = Fiber.current
      @__count__ = 0
    end
    @__count__ += 1
    nil
  end

  def exit
    mon_check_owner
    @__count__ -= 1
    if @__count__ == 0
      @__owner__ = nil
      @__mutex__.unlock
    end
    nil
  end

  def synchronize
    enter
    begin
      yield
    ensure
      self.exit
    end
  end

  def mon_locked? = @__mutex__.locked?
  def mon_owned? = @__mutex__.locked? && __owned__?

  def mon_check_owner
    Kernel.raise ThreadError, "current fiber not owner" unless __owned__?
    nil
  end

  # Wait on `cond` with the monitor released however deep it was entered,
  # and take it back as deep after, whatever ended the wait.
  def wait_for_cond(cond, timeout)
    count = @__count__
    @__owner__ = nil
    @__count__ = 0
    begin
      signaled = cond.wait(@__mutex__, timeout)
    ensure
      @__owner__ = Fiber.current
      @__count__ = count
    end
    signaled ? true : false
  end

  def __owned__? = @__owner__.equal?(Fiber.current)
  private :__owned__?
end

# Upstream's file starts with a magic comment leaving string literals
# unfrozen, the default; here, after the half above, `ruby -w` would only
# warn that it is ignored, so it is left out.
#
# = monitor.rb
#
# Copyright (C) 2001  Shugo Maeda <shugo@ruby-lang.org>
#
# This library is distributed under the terms of the Ruby license.
# You can freely distribute/modify this library.
#


#
# In concurrent programming, a monitor is an object or module intended to be
# used safely by more than one thread. The defining characteristic of a
# monitor is that its methods are executed with mutual exclusion. That is, at
# each point in time, at most one thread may be executing any of its methods.
# This mutual exclusion greatly simplifies reasoning about the implementation
# of monitors compared to reasoning about parallel code that updates a data
# structure.
#
# You can read more about the general principles on the Wikipedia page for
# Monitors[https://en.wikipedia.org/wiki/Monitor_%28synchronization%29].
#
# == Examples
#
# === Simple object.extend
#
#   require 'monitor.rb'
#
#   buf = []
#   buf.extend(MonitorMixin)
#   empty_cond = buf.new_cond
#
#   # consumer
#   Thread.start do
#     loop do
#       buf.synchronize do
#         empty_cond.wait_while { buf.empty? }
#         print buf.shift
#       end
#     end
#   end
#
#   # producer
#   while line = ARGF.gets
#     buf.synchronize do
#       buf.push(line)
#       empty_cond.signal
#     end
#   end
#
# The consumer thread waits for the producer thread to push a line to buf
# while <tt>buf.empty?</tt>. The producer thread (main thread) reads a
# line from ARGF and pushes it into buf then calls <tt>empty_cond.signal</tt>
# to notify the consumer thread of new data.
#
# === Simple Class include
#
#   require 'monitor'
#
#   class SynchronizedArray < Array
#
#     include MonitorMixin
#
#     def initialize(*args)
#       super(*args)
#     end
#
#     alias :old_shift :shift
#     alias :old_unshift :unshift
#
#     def shift(n=1)
#       self.synchronize do
#         self.old_shift(n)
#       end
#     end
#
#     def unshift(item)
#       self.synchronize do
#         self.old_unshift(item)
#       end
#     end
#
#     # other methods ...
#   end
#
# +SynchronizedArray+ implements an Array with synchronized access to items.
# This Class is implemented as subclass of Array which includes the
# MonitorMixin module.
#
module MonitorMixin
  #
  # FIXME: This isn't documented in Nutshell.
  #
  # Since MonitorMixin.new_cond returns a ConditionVariable, and the example
  # above calls while_wait and signal, this class should be documented.
  #
  class ConditionVariable
    #
    # Releases the lock held in the associated monitor and waits; reacquires the lock on wakeup.
    #
    # If +timeout+ is given, this method returns after +timeout+ seconds passed,
    # even if no other thread doesn't signal.
    #
    def wait(timeout = nil)
      @monitor.mon_check_owner
      @monitor.wait_for_cond(@cond, timeout)
    end

    #
    # Calls wait repeatedly while the given block yields a truthy value.
    #
    def wait_while
      while yield
        wait
      end
    end

    #
    # Calls wait repeatedly until the given block yields a truthy value.
    #
    def wait_until
      until yield
        wait
      end
    end

    #
    # Wakes up the first thread in line waiting for this lock.
    #
    def signal
      @monitor.mon_check_owner
      @cond.signal
    end

    #
    # Wakes up all threads waiting for this lock.
    #
    def broadcast
      @monitor.mon_check_owner
      @cond.broadcast
    end

    private

    def initialize(monitor) # :nodoc:
      @monitor = monitor
      @cond = Thread::ConditionVariable.new
    end
  end

  def self.extend_object(obj) # :nodoc:
    super(obj)
    obj.__send__(:mon_initialize)
  end

  #
  # Attempts to enter exclusive section.  Returns +false+ if lock fails.
  #
  def mon_try_enter
    @mon_data.try_enter
  end
  # For backward compatibility
  alias try_mon_enter mon_try_enter

  #
  # Enters exclusive section.
  #
  def mon_enter
    @mon_data.enter
  end

  #
  # Leaves exclusive section.
  #
  def mon_exit
    mon_check_owner
    @mon_data.exit
  end

  #
  # Returns true if this monitor is locked by any thread
  #
  def mon_locked?
    @mon_data.mon_locked?
  end

  #
  # Returns true if this monitor is locked by current thread.
  #
  def mon_owned?
    @mon_data.mon_owned?
  end

  #
  # Enters exclusive section and executes the block.  Leaves the exclusive
  # section automatically when the block exits.  See example under
  # +MonitorMixin+.
  #
  def mon_synchronize(&b)
    @mon_data.synchronize(&b)
  end
  alias synchronize mon_synchronize

  #
  # Creates a new MonitorMixin::ConditionVariable associated with the
  # Monitor object.
  #
  def new_cond
    unless defined?(@mon_data)
      mon_initialize
      @mon_initialized_by_new_cond = true
    end
    return ConditionVariable.new(@mon_data)
  end

  private

  # Use <tt>extend MonitorMixin</tt> or <tt>include MonitorMixin</tt> instead
  # of this constructor.  Have look at the examples above to understand how to
  # use this module.
  def initialize(...)
    super
    mon_initialize
  end

  # Initializes the MonitorMixin after being included in a class or when an
  # object has been extended with the MonitorMixin
  def mon_initialize
    if defined?(@mon_data)
      if defined?(@mon_initialized_by_new_cond)
        return # already initialized.
      elsif @mon_data_owner_object_id == self.object_id
        raise ThreadError, "already initialized"
      end
    end
    @mon_data = ::Monitor.new
    @mon_data_owner_object_id = self.object_id
  end

  # Ensures that the MonitorMixin is owned by the current thread,
  # otherwise raises an exception.
  def mon_check_owner
    @mon_data.mon_check_owner
  end
end

# Use the Monitor class when you want to have a lock object for blocks with
# mutual exclusion.
#
#   require 'monitor'
#
#   lock = Monitor.new
#   lock.synchronize do
#     # exclusive access
#   end
#
class Monitor
  #
  # Creates a new MonitorMixin::ConditionVariable associated with the
  # Monitor object.
  #
  def new_cond
    ::MonitorMixin::ConditionVariable.new(self)
  end

  # for compatibility
  alias try_mon_enter try_enter
  alias mon_try_enter try_enter
  alias mon_enter enter
  alias mon_exit exit
  alias mon_synchronize synchronize
end

# Documentation comments:
#  - All documentation comes from Nutshell.
#  - MonitorMixin.new_cond appears in the example, but is not documented in
#    Nutshell.
#  - All the internals (internal modules Accessible and Initializable, class
#    ConditionVariable) appear in RDoc.  It might be good to hide them, by
#    making them private, or marking them :nodoc:, etc.
#  - RDoc doesn't recognise aliases, so we have mon_synchronize documented, but
#    not synchronize.
#  - mon_owner is in Nutshell, but appears as an accessor in a separate module
#    here, so is hard/impossible to RDoc.  Some other useful accessors
#    (mon_count and some queue stuff) are also in this module, and don't appear
#    directly in the RDoc output.
#  - in short, it may be worth changing the code layout in this file to make the
#    documentation easier
