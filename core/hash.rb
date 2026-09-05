# Hash.
#
# `Class#allocate` gives a Hash one slot holding an association list — an Array
# of `[key, value]` pairs — and every method here is a linear walk over it.
#
# ponytail: O(n) lookup. A real Hash is an open-addressed table keyed by
# `#hash`, and it is worth writing when a spec can construct a hash to measure:
# hash literals are not compiled yet (#157), so today the only way to build one
# is `Hash.new` plus `[]=`. The upgrade is `core/hash.rb` and one hashing
# primitive; nothing outside this file knows the representation.
class Hash
  # `Hash.new`, `Hash.new(0)`, `Hash.new { |h, k| ... }`, and the `capacity:`
  # keyword, which is a sizing hint this representation has no use for.
  def initialize(*default, capacity: 0, &blk)
    # A block *and* a positional default is an error even when that default is
    # `nil`: `Hash.new(nil) { 0 }` raises. So this counts arguments given rather
    # than testing one for nil.
    if !blk.nil? && default.size > 0
      raise ArgumentError, "wrong number of arguments (given " + default.size.to_s + ", expected 0)"
    end
    if default.size > 1
      raise ArgumentError, "wrong number of arguments (given " + default.size.to_s + ", expected 0..1)"
    end
    __set_default__(blk.nil? ? default[0] : blk)
    __set_default_is_proc__(!blk.nil?)
    self
  end

  def default
    __default_is_proc__ ? nil : __default__
  end

  def default_proc
    __default_is_proc__ ? __default__ : nil
  end

  def size
    __pairs__.size
  end

  def length
    size
  end

  def empty?
    size == 0
  end

  def __index__(key)
    pairs = __pairs__
    i = 0
    while i < pairs.size
      return i if pairs[i][0] == key
      i = i + 1
    end
    nil
  end

  def [](key)
    at = __index__(key)
    return __pairs__[at][1] unless at.nil?
    return __default__.call(self, key) if __default_is_proc__
    __default__
  end

  def fetch(key, fallback = nil)
    at = __index__(key)
    return __pairs__[at][1] unless at.nil?
    return yield(key) if block_given?
    return fallback unless fallback.nil?
    raise KeyError, "key not found: " + key.inspect
  end

  def []=(key, value)
    at = __index__(key)
    if at.nil?
      __pairs__.push([key, value])
    else
      __pairs__[at][1] = value
    end
    value
  end

  def store(key, value)
    self[key] = value
  end

  def key?(key)
    !__index__(key).nil?
  end

  def has_key?(key)
    key?(key)
  end

  def include?(key)
    key?(key)
  end

  def member?(key)
    key?(key)
  end

  def value?(value)
    values.include?(value)
  end

  def keys
    __pairs__.map { |pair| pair[0] }
  end

  def values
    __pairs__.map { |pair| pair[1] }
  end

  def each
    __pairs__.each { |pair| yield pair[0], pair[1] }
    self
  end

  def each_pair
    __pairs__.each { |pair| yield pair[0], pair[1] }
    self
  end

  def each_key
    keys.each { |key| yield key }
    self
  end

  def each_value
    values.each { |value| yield value }
    self
  end

  def delete(key)
    at = __index__(key)
    return nil if at.nil?
    pairs = __pairs__
    gone = pairs[at][1]
    kept = []
    i = 0
    while i < pairs.size
      kept.push(pairs[i]) unless i == at
      i = i + 1
    end
    __set_pairs__(kept)
    gone
  end

  def to_a
    __pairs__.map { |pair| [pair[0], pair[1]] }
  end

  def inspect
    return "{}" if empty?
    out = "{"
    pairs = __pairs__
    i = 0
    while i < pairs.size
      out = out + pairs[i][0].inspect + " => " + pairs[i][1].inspect
      out = out + ", " if i < pairs.size - 1
      i = i + 1
    end
    out + "}"
  end

  def to_s
    inspect
  end
end
