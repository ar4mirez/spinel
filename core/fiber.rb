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
    __fiber_new__(self, block)
  end

  def resume(*args)
    __fiber_resume__(self, args)
  end

  def transfer(*args)
    __fiber_transfer__(self, args)
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
