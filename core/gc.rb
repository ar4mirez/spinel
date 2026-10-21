# GC and ObjectSpace (#36): the collector, as Ruby sees it.
#
# The collector is `Heap::collect` in `heap.rs`, a precise, non-moving
# mark-sweep with one generation. What it cannot be asked from Ruby is in the
# `__gc_*__`, `__weak_*__` and `__finalizer_*__` primitives; the rest is here.
module GC
  # Collects even when `GC.disable` is in force, as CRuby's does. The
  # collection itself happens as this method returns: see `interp.rs`, where
  # the loop notices a call or a return. The keywords pick a kind of
  # collection, and this collector has one kind.
  def self.start(full_mark: true, immediate_sweep: true)
    __gc_start__
    nil
  end

  def garbage_collect(full_mark: true, immediate_sweep: true)
    GC.start
    nil
  end

  def self.count
    stat(:count)
  end

  # Every key CRuby 4.0 has, so a program reading one gets an Integer.
  # With a Hash, its entries are updated in place and it is the answer.
  def self.stat(key = nil)
    flat = __gc_stat__
    case key
    when nil, Hash
      hash = key || {}
      i = 0
      while i < flat.size
        hash[flat[i]] = flat[i + 1]
        i += 2
      end
      hash
    when Symbol
      i = 0
      while i < flat.size
        return flat[i + 1] if flat[i].equal?(key)
        i += 2
      end
      raise ArgumentError, "unknown key: #{key}"
    else
      raise TypeError, "non-hash or symbol given"
    end
  end

  # One heap with no size pools, so there is nothing to report per pool.
  def self.stat_heap(heap_name = nil, key = nil)
    heap_name.nil? || Hash === heap_name ? (heap_name || {}) : nil
  end

  def self.latest_gc_info(key = nil)
    info = {
      major_by: :force, need_major_by: nil, gc_by: :method, have_finalizer: false,
      immediate_sweep: true, state: :none, weak_references_count: 0, retained_weak_references_count: 0,
    }
    case key
    when nil then info
    when Hash then key.update(info)
    when Symbol
      raise ArgumentError, "unknown key: #{key}" unless info.key?(key)
      info[key]
    else
      raise TypeError, "non-hash or symbol given"
    end
  end

  def self.disable
    __gc_disable__(true)
  end

  def self.enable
    __gc_disable__(false)
  end

  def self.stress
    __gc_stress__
  end

  def self.stress=(flag)
    __gc_stress__(flag)
    flag
  end

  # Nanoseconds spent collecting. ponytail: it is measured whether or not
  # `measure_total_time` is on, where CRuby stops the clock; the flag only
  # answers back what it was set to. Honour it if a program is found that
  # turns measuring off to save the cost of a clock read per collection.
  def self.total_time
    __gc_time__
  end

  def self.measure_total_time
    @__measure_total_time__ = true if @__measure_total_time__.nil?
    @__measure_total_time__
  end

  def self.measure_total_time=(flag)
    @__measure_total_time__ = flag ? true : false
    flag
  end

  # The collector does not move objects, so there is nothing to compact,
  # and saying so is CRuby's answer on a platform that cannot.
  def self.auto_compact
    false
  end

  def self.auto_compact=(flag)
    raise NotImplementedError, "Automatic compaction isn't available on this platform" if flag
    flag
  end

  def self.compact
    raise NotImplementedError, "Compaction isn't available on this platform"
  end

  # The implementation's name, and no settings: this collector has none a
  # program can change.
  def self.config(options = nil)
    config = { implementation: "spinel" }
    return config if options.nil?
    raise ArgumentError, "ArgumentError" unless Hash === options
    if options.key?(:implementation)
      raise ArgumentError, 'Attempting to set read-only key "Implementation"'
    end
    config
  end

  # The profiler records nothing yet: `result` is the empty report CRuby
  # gives when none was taken. ponytail: a row per collection is
  # `Heap::collect` timing itself into a list the profiler reads, which is
  # worth writing for a program that prints `GC::Profiler.report`.
  module Profiler
    def self.enabled?
      @__enabled__ ? true : false
    end

    def self.enable
      @__enabled__ = true
      nil
    end

    def self.disable
      @__enabled__ = false
      nil
    end

    def self.clear
      nil
    end

    def self.raw_data
      []
    end

    def self.result
      +""
    end

    def self.report(out = $stdout)
      out.write(result)
      nil
    end

    def self.total_time
      0.0
    end
  end
end

module ObjectSpace
  def self.garbage_collect(full_mark: true, immediate_sweep: true)
    GC.start
    nil
  end

  def garbage_collect(full_mark: true, immediate_sweep: true)
    GC.start
    nil
  end

  # Every object with a class that is a kind of `mod`, which is every object
  # Ruby can reach — the core library's own included. The walk is a snapshot
  # taken before the first block runs.
  def self.each_object(mod = BasicObject)
    raise TypeError, "class or module required" if __reflect_module_kind__(mod).nil?
    return to_enum(:each_object, mod) unless block_given?
    objects = __each_object__(mod)
    objects.each { |object| yield object }
    objects.size
  end

  # ponytail: only the totals and the types the header can tell apart, where
  # CRuby counts a dozen internal ones. Enough for a program that reads
  # `:TOTAL` and `:FREE`; the rest wait on a program that reads them.
  def self.count_objects(result = {})
    raise TypeError, "non-hash given" unless Hash === result
    total = GC.stat(:heap_available_slots)
    live = GC.stat(:heap_live_slots)
    result[:TOTAL] = total
    result[:FREE] = total - live < 0 ? 0 : total - live
    result
  end

  # Deprecated in Ruby 4.0, and still answers.
  def self._id2ref(id)
    warn("ObjectSpace._id2ref is deprecated", uplevel: 1, category: :deprecated)
    id = Integer.__index__(id)
    return nil if id == 8
    return true if id == 20
    return false if id == 0
    missing = Object.new
    found = __id2ref__(id, missing)
    return found unless found.equal?(missing)
    return (id - 1) / 2 if id.odd? && id.abs < (1 << 62)
    raise RangeError, "#{id} is not an id value"
  end

  # Registers `callable` to be called with `obj`'s `object_id` once `obj`
  # is collected, or when the program ends if it never is. Answers
  # `[0, callable]`, and a callable `==` to one already registered for `obj`
  # is not registered twice.
  def self.define_finalizer(obj, callable = (no_callable = true; nil), &block)
    callable = block if no_callable
    raise ArgumentError, "tried to create Proc object without a block" if no_callable && block.nil?
    unless callable.respond_to?(:call, true)
      raise ArgumentError, "wrong type argument #{Kernel.__class_of__(callable)} (should be callable)"
    end
    if __immediate__(obj)
      raise ArgumentError, "cannot define finalizer for #{Kernel.__class_of__(obj)}"
    end
    raise FrozenError.new("can't modify frozen #{Kernel.__class_of__(obj)}: #{obj.inspect}", receiver: obj) if obj.frozen?
    if __finalizer_captures__(obj, callable)
      warn("finalizer references object to be finalized", uplevel: 1)
    end
    existing = __finalizer_list__(obj).find { |kept| kept == callable }
    return [0, existing] unless existing.nil?
    __finalizer_add__(obj, callable)
    [0, callable]
  end

  def self.undefine_finalizer(obj)
    raise FrozenError.new("can't modify frozen #{Kernel.__class_of__(obj)}: #{obj.inspect}", receiver: obj) if obj.frozen?
    __finalizer_remove__(obj)
    obj
  end

  # Whether a finalizer would keep its own object alive: a block whose
  # `self` is the object, or a method bound to it. CRuby warns about those,
  # because such an object is never collected and its finalizer only runs
  # at exit.
  def self.__finalizer_captures__(obj, callable)
    case callable
    when Proc then callable.binding.receiver.equal?(obj)
    when Method then callable.receiver.equal?(obj)
    else false
    end
  rescue Exception
    false
  end

  # Run finalizers, given as `[id, [callable, ...], ...]`. An exception in
  # one is a warning, unless `$VERBOSE` is nil, and the rest still run.
  def self.__finalize__(list)
    i = 0
    while i < list.size
      id = list[i]
      list[i + 1].each do |callable|
        callable.call(id)
      rescue Exception => e
        unless $VERBOSE.nil?
          $stderr.write("#{$0}: warning: Exception in finalizer #{callable.inspect}\n")
          $stderr.write(e.full_message)
        end
      end
      i += 2
    end
    nil
  end

  # The program is ending: every finalizer still registered runs, then any
  # those registered, until none is left. `exit` inside one ends only that
  # one, as an `at_exit` block's does.
  def self.__finalize_all__
    until (list = __finalizer_take__(true)).empty?
      begin
        __finalize__(list)
      rescue SystemExit
      end
    end
    nil
  end

  # Holds its keys and values weakly, comparing keys by identity: an entry
  # goes once either half has been collected. An immediate never is.
  class WeakMap
    include Enumerable

    def initialize
      __weak_init__
    end

    MISSING = Object.new
    private_constant :MISSING

    def [](key)
      found = __weak_get__(key, MISSING)
      found.equal?(MISSING) ? nil : found
    end

    def []=(key, value)
      __weak_set__(key, value, 0)
    end

    def delete(key)
      found = __weak_delete__(key, MISSING)
      return found unless found.equal?(MISSING)
      block_given? ? yield(key) : nil
    end

    def key?(key)
      !__weak_get__(key, MISSING).equal?(MISSING)
    end
    alias include? key?
    alias member? key?

    def size
      __weak_size__
    end
    alias length size

    def each
      pairs = __weak_pairs__
      i = 0
      while i < pairs.size
        yield pairs[i], pairs[i + 1]
        i += 2
      end
      self
    end
    alias each_pair each

    def each_key
      each { |key, _| yield key }
    end

    def each_value
      each { |_, value| yield value }
    end

    def keys
      pairs = __weak_pairs__
      (0...pairs.size).step(2).map { |i| pairs[i] }
    end

    def values
      pairs = __weak_pairs__
      (1...pairs.size).step(2).map { |i| pairs[i] }
    end

    # Each half as CRuby's `rb_any_to_s` writes it, unless it is an
    # immediate: an object's own `inspect` is never called, so a
    # `BasicObject` that has none is shown too.
    def inspect
      head = "#<#{Kernel.__class_of__(self)}:0x#{__address__}"
      pairs = __weak_pairs__
      return head + ">" if pairs.empty?
      shown = (0...pairs.size).step(2).map do |i|
        "#{WeakMap.__show__(pairs[i])} => #{WeakMap.__show__(pairs[i + 1])}"
      end
      "#{head}: #{shown.join(", ")}>"
    end

    def self.__show__(object)
      return object.inspect if __immediate__(object)
      hex = object.__id__.to_s(16)
      hex = "0" + hex while hex.length < 16
      "#<#{Kernel.__class_of__(object)}:0x#{hex}>"
    end
  end

  # Holds its keys weakly and compares them with `hash` and `eql?`, as a
  # Hash does; a value lives as long as its key. A key has to be an object
  # the collector can collect, so an immediate is refused.
  class WeakKeyMap
    def initialize
      __weak_init__
    end

    def [](key)
      found = __key__(key)
      found.nil? ? nil : __weak_get__(found[0], nil)
    end

    def []=(key, value)
      if __immediate__(key)
        raise ArgumentError, "WeakKeyMap keys must be garbage collectable"
      end
      found = __key__(key)
      if found.nil?
        __weak_set__(key, value, key.__send__(:hash))
      else
        __weak_set__(found[0], value, found[1])
      end
      value
    end

    def delete(key)
      found = __key__(key)
      return __weak_delete__(found[0], nil) unless found.nil?
      block_given? ? yield(key) : nil
    end

    def getkey(key)
      found = __key__(key)
      found.nil? ? nil : found[0]
    end

    def key?(key)
      !__key__(key).nil?
    end

    def clear
      __weak_clear__
      self
    end

    def inspect
      "#<#{Kernel.__class_of__(self)}:0x#{__address__} size=#{__weak_size__}>"
    end

    # The key this map stored that `key` finds, with its hash, or nil: one
    # with the same `hash`, the identical object first, then by `eql?`.
    def __key__(key)
      return nil if __immediate__(key)
      hash = key.__send__(:hash)
      candidates = __weak_hashed__(hash)
      found = candidates.find { |kept| kept.equal?(key) } || candidates.find { |kept| key.eql?(kept) }
      found.nil? ? nil : [found, hash]
    end
    private :__key__
  end
end

module Kernel
  # What the interpreter calls when a collection has queued finalizers.
  def self.__run_finalizers__(list)
    ObjectSpace.__finalize__(list)
  end
end
