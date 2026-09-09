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
  # `Hash[...]` takes one Array of pairs, one Hash, or an even-length flat
  # argument list. An odd flat list is an ArgumentError, not a dropped value.
  def self.[](*given)
    return __from_pairs_or_hash__(given[0]) if given.size == 1
    if given.size % 2 != 0
      raise ArgumentError, "odd number of arguments for Hash"
    end
    out = {}
    i = 0
    while i < given.size
      out[given[i]] = given[i + 1]
      i = i + 2
    end
    out
  end

  def self.__from_pairs_or_hash__(source)
    if source.is_a?(Hash)
      out = {}
      source.each_pair { |k, v| out[k] = v }
      return out
    end
    unless source.is_a?(Array)
      raise ArgumentError, "odd number of arguments for Hash"
    end
    out = {}
    source.each do |pair|
      unless pair.is_a?(Array) && pair.size >= 1 && pair.size <= 2
        raise ArgumentError, "invalid number of elements"
      end
      out[pair[0]] = pair.size == 2 ? pair[1] : nil
    end
    out
  end

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
  # A copy has to get its own `@pairs`, and its own pair arrays inside it.
  #
  # `Kernel#dup` is a shallow copy of the object's slots, which for `Array` is
  # its elements and for `Hash` is one ivar *pointing at* an Array — so without
  # this the copy and the original share one table and `h.dup[:b] = 2` mutates
  # `h`. Silently, with the right-looking answer for every read.
  #
  # ponytail: `dup` and `clone` are overridden to call this because
  # `Kernel#dup` does not call `initialize_copy` yet (#201). When it does, both
  # overrides delete and this method stays exactly as it is.
  def initialize_copy(other)
    pairs = []
    other.each_pair { |key, value| pairs.push([key, value]) }
    @pairs = pairs
    __copy_default_from__(other)
    self
  end

  def dup
    self.class.allocate.__init_copy_of__(self, false)
  end

  # ponytail: `clone` carries the frozen state and not the singleton class.
  # `Kernel#clone` copies both, and this cannot reach the second — a copy that
  # shares its table is a worse answer than one that loses a singleton method,
  # and both go away with #201, when `Kernel#clone` calls `initialize_copy` and
  # these two overrides delete.
  def clone(freeze: nil)
    copy = self.class.allocate.__init_copy_of__(self, true)
    copy.freeze if freeze.nil? ? frozen? : freeze
    copy
  end

  def __init_copy_of__(other, keep_frozen)
    @pairs = []
    @default = nil
    @default_is_proc = false
    initialize_copy(other)
    self
  end

  def default(*key)
    return @default unless @default_is_proc
    key.empty? ? nil : @default.call(self, key[0])
  end

  def default=(value)
    __check_frozen__
    @default = value
    @default_is_proc = false
    value
  end

  def default_proc
    @default_is_proc ? @default : nil
  end

  # Two checks Ruby makes and the messages it makes them with, measured: a
  # non-Proc that cannot be converted is "wrong default_proc type X (expected
  # Proc)", and a *lambda* of the wrong arity is "default_proc takes two
  # arguments (2 for N)". A plain proc is not arity-checked, because a proc
  # ignores extra arguments anyway.
  def default_proc=(block)
    __check_frozen__
    if block.nil?
      @default = nil
      @default_is_proc = false
      return nil
    end
    unless block.is_a?(Proc)
      unless block.respond_to?(:to_proc)
        raise TypeError, "wrong default_proc type " + block.class.name + " (expected Proc)"
      end
      block = block.to_proc
      unless block.is_a?(Proc)
        raise TypeError, "wrong default_proc type " + block.class.name + " (expected Proc)"
      end
    end
    if block.lambda? && block.arity != 2
      raise TypeError, "default_proc takes two arguments (2 for " + block.arity.to_s + ")"
    end
    @default = block
    @default_is_proc = true
    block
  end

  # Which of the two a hash carries, copied whole. `compact` and `replace` keep
  # the default; `Hash[]`, `except` and `slice` deliberately do not — measured,
  # and the reason those three build a fresh hash rather than `dup` one.
  def __copy_default_from__(other)
    @default = other.__raw_default__
    @default_is_proc = other.__default_is_proc__
    self
  end

  def __raw_default__
    @default
  end

  def __default_is_proc__
    @default_is_proc
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
  # --- merging ---------------------------------------------------------------

  # `merge` answers a new Hash even with no arguments at all — measured:
  # `h.merge.equal?(h)` is false. The block sees `(key, old, new)` and its value
  # becomes the entry, and it only fires on a key both sides hold.
  def merge(*others, &block)
    out = dup
    out.__merge_from__(others, block)
    out
  end

  def merge!(*others, &block)
    __check_frozen__
    __merge_from__(others, block)
    self
  end

  def update(*others, &block)
    merge!(*others, &block)
  end

  def __merge_from__(others, block)
    i = 0
    while i < others.size
      other = others[i]
      unless other.is_a?(Hash)
        unless other.respond_to?(:to_hash)
          raise TypeError, "no implicit conversion of " + other.class.name + " into Hash"
        end
        other = other.to_hash
      end
      # Snapshot first: `h.merge!(h)` iterates the hash it is writing to, and
      # reading `self[key]` back after an assignment would hand the block the
      # value it just produced. `merge_spec.rb` pins that `merge!` and `merge`
      # see the same entries in the same order.
      pairs = other.to_a
      j = 0
      while j < pairs.size
        key = pairs[j][0]
        value = pairs[j][1]
        self[key] = block.nil? || !key?(key) ? value : block.call(key, self[key], value)
        j = j + 1
      end
      i = i + 1
    end
    self
  end

  def replace(other)
    __check_frozen__
    unless other.is_a?(Hash)
      raise TypeError, "no implicit conversion of " + other.class.name + " into Hash"
    end
    clear
    other.each_pair { |key, value| self[key] = value }
    __copy_default_from__(other)
    self
  end

  def clear
    __check_frozen__
    keys.each { |key| delete(key) }
    self
  end

  # --- lookup ----------------------------------------------------------------

  # `==`, not the table's `eql?`: `{1.0 => :v}.assoc(1)` finds the entry where
  # `key?(1)` would not, because `1.0.eql?(1)` is false while `1.0 == 1` is
  # true. So this scans rather than indexes.
  def assoc(key)
    each_pair { |k, v| return [k, v] if k == key }
    nil
  end

  def rassoc(value)
    each_pair { |k, v| return [k, v] if v == value }
    nil
  end

  # A step that is present but cannot be dug into is a TypeError naming the
  # class, not a nil — measured: `{a: 1}.dig(:a, :b)` says
  # "Integer does not have #dig method". A step that is *absent* stops at nil.
  def dig(*path)
    if path.empty?
      raise ArgumentError, "wrong number of arguments (given 0, expected 1+)"
    end
    value = self[path[0]]
    i = 1
    while i < path.size
      return nil if value.nil?
      unless value.respond_to?(:dig)
        raise TypeError, value.class.name + " does not have #dig method"
      end
      value = value.dig(path[i])
      i = i + 1
    end
    value
  end

  def values_at(*wanted)
    out = []
    wanted.each { |key| out.push(self[key]) }
    out
  end

  def fetch_values(*wanted, &block)
    out = []
    wanted.each { |key| out.push(block.nil? ? fetch(key) : fetch(key, &block)) }
    out
  end

  # `__index__` rather than `[]`: Ruby uses the regular reader even on a
  # subclass that overrides `[]`, which `slice_spec.rb` pins.
  def slice(*wanted)
    out = {}
    wanted.each { |key| out[key] = __index__(key) if key?(key) }
    out
  end

  def except(*unwanted)
    out = {}
    each_pair { |k, v| out[k] = v }
    unwanted.each { |key| out.delete(key) }
    out
  end

  # --- in place --------------------------------------------------------------

  # `reject!`, `select!` and `compact!` answer nil when they changed nothing,
  # while `delete_if` and `keep_if` always answer self. That is the only
  # difference between the two pairs, and it is measured rather than guessed.
  def reject!(&block)
    return to_enum(:reject!) if block.nil?
    __check_frozen__
    removed = __remove_where__(block, true)
    removed ? self : nil
  end

  def delete_if(&block)
    return to_enum(:delete_if) if block.nil?
    __check_frozen__
    __remove_where__(block, true)
    self
  end

  def select!(&block)
    return to_enum(:select!) if block.nil?
    __check_frozen__
    removed = __remove_where__(block, false)
    removed ? self : nil
  end

  def filter!(&block)
    select!(&block)
  end

  def keep_if(&block)
    return to_enum(:keep_if) if block.nil?
    __check_frozen__
    __remove_where__(block, false)
    self
  end

  # Collects first: deleting while iterating is not something this table
  # promises to survive.
  def __remove_where__(block, when_true)
    doomed = []
    each_pair { |k, v| doomed.push(k) if block.call(k, v) == when_true }
    doomed.each { |key| delete(key) }
    !doomed.empty?
  end

  def compact
    out = {}
    out.__copy_default_from__(self)
    each_pair { |k, v| out[k] = v unless v.nil? }
    out
  end

  def compact!
    removed = __remove_where__(->(_k, v) { v.nil? }, true)
    removed ? self : nil
  end

  # --- shaping ---------------------------------------------------------------

  def invert
    out = {}
    each_pair { |k, v| out[v] = k }
    out
  end

  # Depth 1 by default, so a value that is an Array stays one: `{a: [1, 2]}
  # .flatten` is `[:a, [1, 2]]`. Depth 0 is the pairs themselves and a negative
  # depth is unbounded.
  # Depth 1 by default, so a value that is an Array stays one: `{a: [1, 2]}
  # .flatten` is `[:a, [1, 2]]`. Depth 0 is the pairs themselves and a negative
  # depth is unbounded.
  #
  # Written out rather than handed to `Array#flatten`, which does not exist yet
  # (#21). Calling it would have raised NoMethodError, and a raising method is
  # reported *blocked* rather than failed — so every `flatten` spec would have
  # stayed green-looking while the method did nothing.
  def flatten(*depth)
    level = depth.empty? ? 1 : __to_int__(depth[0])
    pairs = to_a
    return pairs if level == 0
    out = []
    pairs.each { |pair| __flatten_into__(out, pair, level) }
    out
  end

  def __flatten_into__(out, value, level)
    if level == 0 || !value.is_a?(Array)
      out.push(value)
      return out
    end
    value.each { |element| __flatten_into__(out, element, level - 1) }
    out
  end

  def transform_values(&block)
    return to_enum(:transform_values) if block.nil?
    out = {}
    each_pair { |k, v| out[k] = block.call(v) }
    out
  end

  def transform_values!(&block)
    __check_frozen__
    each_pair { |k, v| self[k] = block.call(v) }
    self
  end

  # With a Hash argument, a key it does not hold is left alone rather than
  # dropped: `{a: 1, b: 2}.transform_keys({a: :x})` is `{x: 1, b: 2}`.
  def transform_keys(*mapping, &block)
    table = mapping.empty? ? nil : mapping[0]
    if !mapping.empty? && table.nil?
      raise TypeError, "no implicit conversion of nil into Hash"
    end
    out = {}
    each_pair do |k, v|
      key = k
      if !table.nil? && table.key?(k)
        key = table[k]
      elsif !block.nil?
        key = block.call(k)
      end
      out[key] = v
    end
    out
  end

  # `h.to_proc` is a lambda, which is observable: `.lambda?` is true.
  def to_proc
    table = self
    ->(key) { table[key] }
  end

  def each_entry(&block)
    return to_enum(:each_entry) if block.nil?
    each_pair { |k, v| block.call([k, v]) }
    self
  end

  def __check_frozen__
    raise FrozenError, "can't modify frozen Hash: " + inspect if frozen?
  end

  # Keys are matched by the table's own rule — `eql?` and `hash` — while values
  # are compared with `==`. Measured, and the two halves disagree:
  # `{1 => 2} == {1.0 => 2}` is false because `1.0.eql?(1)` is not, while
  # `{a: 1} == {a: 1.0}` is true because `1 == 1.0` is. Order does not matter.
  #
  # `eql?` is the same walk with `eql?` on the values too, which is what makes
  # `{a: 1}.eql?({a: 1.0})` false where `==` is true.
  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Hash)
    __same_pairs__(other, false)
  end

  def eql?(other)
    return true if equal?(other)
    return false unless other.is_a?(Hash)
    __same_pairs__(other, true)
  end

  # ponytail: `Hash#hash` is deliberately absent, so it stays `Kernel#hash`'s
  # identity. A content digest is what `eql?` needs to be usable as a key, and
  # writing one here hangs: `hash_spec.rb` builds `h[:a] = h` and asks for it,
  # and a recursive walk over a self-referential table does not terminate.
  # CRuby guards that with `rb_exec_recursive`; doing it in Ruby needs a
  # threaded seen-list *and* an `Array#hash` to join it, because the spec also
  # recurses through `h[:x] = [h]` — and `Array#hash` is #21's.
  #
  # A wrong-but-terminating digest would make `{a: 1}` findable as a key and
  # break silently the first time two different hashes collided, so identity
  # stays until both halves exist. `==` and `eql?` above terminate on the same
  # structures only because they short-circuit on `equal?` first.

  def __same_pairs__(other, strict)
    return false unless size == other.size
    each_pair do |key, value|
      return false unless other.key?(key)
      theirs = other[key]
      return false unless strict ? value.eql?(theirs) : value == theirs
    end
    true
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
