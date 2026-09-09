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
  include Enumerable

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

  # What a `**kw` parameter is handed (#193).
  #
  # The binder is Rust and cannot send, so it leaves the keywords no named
  # parameter claimed as an `Array` of `[key, value]` pairs; the method's
  # prologue calls this. In pair order, which is the call's source order.
  def self.__from_pairs__(pairs)
    out = allocate
    i = 0
    while i < pairs.size
      pair = pairs[i]
      out[pair[0]] = pair[1]
      i = i + 1
    end
    out
  end

  # `{ **other }`. Ruby converts with `to_hash`, so a Hash is used as it is and
  # anything else is asked to become one.
  def __merge_literal__(other)
    # `**nil` contributes nothing, and so does a `**h` whose `h` is nil —
    # measured, both answer `{}` rather than raising. Anything else that is not
    # a Hash still raises, which is why this is a nil check and not a rescue:
    # `{**5}` is `no implicit conversion of Integer into Hash`.
    return self if other.nil?

    source = other.respond_to?(:to_hash) ? other.to_hash : other
    unless source.is_a?(Hash)
      raise TypeError, "no implicit conversion of " + other.class.name + " into Hash"
    end
    source.each_pair { |key, value| self[key] = value }
    self
  end

  # `default` and `default(key)` are the same method: with a key it runs the
  # default block, without one it answers nil where a block is what was set.
  # Measured — `Hash.new { |h, k| k }.default` is nil and `.default(:k)` is `:k`.
  def default(*key)
    return @default unless @default_is_proc
    key.empty? ? nil : @default.call(self, key[0])
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

  # A miss goes through `default`, which is a method rather than the ivar: a
  # `Hash` subclass overriding `default(key)` is how ruby/spec's `DefaultHash`
  # answers 100 for every key, and reading `@default` here would never see it.
  def [](key)
    at = __index__(key)
    return @pairs[at][1] unless at.nil?
    default(key)
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
    __check_frozen__
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

  # `to_h` hands the block the key and the value as two arguments, where
  # Enumerable's would hand it the one pair. Without a block it answers the
  # receiver itself, not a copy. Measured.
  def to_h
    return self unless block_given?
    out = {}
    each_pair do |pair|
      made = yield(pair[0], pair[1])
      unless made.is_a?(Array)
        raise TypeError, "wrong element type #{made.class} (expected array)"
      end
      unless made.size == 2
        raise ArgumentError, "element has wrong array length (expected 2, was #{made.size})"
      end
      out[made[0]] = made[1]
    end
    out
  end

  # `select`, `filter` and `reject` answer a Hash, not the Array of pairs
  # Enumerable would build. Hash overrides exactly these three — `find_all`,
  # `filter_map`, `map` and `partition` all still answer Arrays. Measured.
  def select
    return to_enum(:select) unless block_given?
    out = {}
    each_pair { |pair| out[pair[0]] = pair[1] if yield(pair[0], pair[1]) }
    out
  end

  def filter
    return to_enum(:filter) unless block_given?
    out = {}
    each_pair { |pair| out[pair[0]] = pair[1] if yield(pair[0], pair[1]) }
    out
  end

  def reject
    return to_enum(:reject) unless block_given?
    out = {}
    each_pair { |pair| out[pair[0]] = pair[1] unless yield(pair[0], pair[1]) }
    out
  end

  def keys
    @pairs.map { |pair| pair[0] }
  end

  def values
    @pairs.map { |pair| pair[1] }
  end

  # Yields one value, the `[key, value]` pair — not two. Measured: a
  # `{ |x| }` block over a hash binds `x` to the pair, and a `{ |k, v| }` one
  # gets its two locals from the block's own auto-splat rather than from here.
  # Yielding two would leave the first shape holding only the key.
  def each
    return to_enum(:each) unless block_given?
    @pairs.each { |pair| yield pair }
    self
  end

  def each_pair
    return to_enum(:each_pair) unless block_given?
    @pairs.each { |pair| yield pair }
    self
  end

  def each_key
    return to_enum(:each_key) unless block_given?
    keys.each { |key| yield key }
    self
  end

  def each_value
    return to_enum(:each_value) unless block_given?
    values.each { |value| yield value }
    self
  end

  # A block is the "not found" answer, and it is called with the key — so
  # `{}.delete(:x) { |k| 5 }` is 5 rather than nil.
  def delete(key)
    __check_frozen__
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

  # A Hash is its own hash pattern subject (#165). The key list is ignored:
  # Ruby's own `Hash#deconstruct_keys` answers the whole hash either way, and
  # the pattern picks what it wants out of it.
  def deconstruct_keys(keys)
    self
  end

  def to_a
    @pairs.map { |pair| [pair[0], pair[1]] }
  end

  # Every mutation checks first, the way `core/regexp.rb` and `core/range.rb`
  # already do. Measured: "can't modify frozen Hash: {}".
  def __check_frozen__
    raise FrozenError, "can't modify frozen Hash: " + inspect if frozen?
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
