# Hash.
#
# `Class#allocate` gives a Hash three instance variables: `@pairs`, an
# association list of `[key, value]` Arrays, plus the default and whether it is
# a block. Every method here is a linear walk over `@pairs`.
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
    @default = blk.nil? ? default[0] : blk
    @default_is_proc = !blk.nil?
    self
  end

  # What a hash literal is built from (#157). `allocate` rather than `new`: the
  # literal has no arguments to check, and `Class#allocate` is already what
  # gives a Hash its empty `@pairs`. `@default` and `@default_is_proc` are unset
  # and so read as nil, which is the same answer `Hash.new` would have left.
  def self.__literal__
    allocate
  end

  # `{ **other }`. Ruby converts with `to_hash`, so a Hash is used as it is and
  # anything else is asked to become one.
  def __merge_literal__(other)
    source = other.respond_to?(:to_hash) ? other.to_hash : other
    source.each_pair { |key, value| self[key] = value }
    self
  end

  def default
    @default_is_proc ? nil : @default
  end

  def default_proc
    @default_is_proc ? @default : nil
  end

  def size
    @pairs.size
  end

  def length
    size
  end

  def empty?
    size == 0
  end

  # Key identity is `eql?`, not `==`. The difference is what keeps `1` and
  # `1.0` distinct keys: they are `==` but not `eql?`, and Ruby's Hash keeps
  # both. Every lookup here goes through this one method, so `[]`, `[]=`,
  # `key?`, `fetch` and `delete` all agree by construction.
  def __index__(key)
    pairs = @pairs
    i = 0
    while i < pairs.size
      return i if pairs[i][0].eql?(key)
      i = i + 1
    end
    nil
  end

  def [](key)
    at = __index__(key)
    return @pairs[at][1] unless at.nil?
    return @default.call(self, key) if @default_is_proc
    @default
  end

  # `fetch(k, nil)` answers nil; only `fetch(k)` with no block raises. So this
  # counts the arguments given rather than testing one for nil.
  def fetch(*fallback)
    if fallback.size < 1 || fallback.size > 2
      raise ArgumentError,
            "wrong number of arguments (given " + fallback.size.to_s + ", expected 1..2)"
    end
    key = fallback[0]
    at = __index__(key)
    return @pairs[at][1] unless at.nil?
    return yield(key) if block_given?
    return fallback[1] if fallback.size > 1
    raise KeyError, "key not found: " + key.inspect
  end

  def []=(key, value)
    at = __index__(key)
    if at.nil?
      @pairs.push([key, value])
    else
      @pairs[at][1] = value
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
    @pairs.map { |pair| pair[0] }
  end

  def values
    @pairs.map { |pair| pair[1] }
  end

  def each
    @pairs.each { |pair| yield pair[0], pair[1] }
    self
  end

  def each_pair
    @pairs.each { |pair| yield pair[0], pair[1] }
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

  # A block is the "not found" answer, and it is called with the key — so
  # `{}.delete(:x) { |k| 5 }` is 5 rather than nil.
  def delete(key)
    at = __index__(key)
    if at.nil?
      return yield(key) if block_given?
      return nil
    end
    pairs = @pairs
    gone = pairs[at][1]
    kept = []
    i = 0
    while i < pairs.size
      kept.push(pairs[i]) unless i == at
      i = i + 1
    end
    @pairs = kept
    gone
  end

  def to_a
    @pairs.map { |pair| [pair[0], pair[1]] }
  end

  def inspect
    return "{}" if empty?
    out = "{"
    pairs = @pairs
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
