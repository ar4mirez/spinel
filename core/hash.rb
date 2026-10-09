# Hash.
#
# `Class#allocate` gives a Hash three instance variables: `@__pairs__`, an
# association list of `[key, value]` Arrays, plus the default and whether it is
# a block. Two more arrive on use: `@__hashes__`, each key's hash code as it was
# when stored (#22), and `@__identity__`. Every method here is a linear
# walk over `@__pairs__`.
#
# ponytail: O(n) lookup. A real Hash is an open-addressed table keyed by
# `#hash`, and `@__hashes__` is already the half of it that decides which slot. The
# upgrade is this file and a primitive over it; outside it, only
# `hash_pairs`, `hash_of_pairs` and `expand_splats` in `interp.rs` know the
# representation, and each says so.
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
    @__default__ = blk.nil? ? default[0] : blk
    @__default_is_proc__ = !blk.nil?
    self
  end

  # What a hash literal is built from (#157). `allocate` rather than `new`: the
  # literal has no arguments to check, and `Class#allocate` is already what
  # gives a Hash its empty `@__pairs__`. `@__default__` and `@__default_is_proc__` are unset
  # and so read as nil, which is the same answer `Hash.new` would have left.
  # `Hash[...]` takes one Array of pairs, one Hash, or an even-length flat
  # argument list. An odd flat list is an ArgumentError, not a dropped value.
  def self.[](*given)
    return __from_pairs_or_hash__(given[0]) if given.size == 1
    if given.size % 2 != 0
      raise ArgumentError, "odd number of arguments for Hash"
    end
    # An instance of the class it was called on, never initialized: measured,
    # `MyHash[...]` is a MyHash and its `initialize` does not run.
    out = allocate
    i = 0
    while i < given.size
      out[given[i]] = given[i + 1]
      i = i + 2
    end
    out
  end

  def self.__from_pairs_or_hash__(source)
    if source.is_a?(Hash)
      out = allocate
      source.each_pair { |k, v| out[k] = v }
      return out
    end
    unless source.is_a?(Array)
      raise ArgumentError, "odd number of arguments for Hash"
    end
    out = allocate
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
  # A copy has to get its own `@__pairs__`, and its own pair arrays inside it.
  #
  # `Kernel#dup` is a shallow copy of the object's slots, which for `Array` is
  # its elements and for `Hash` is one ivar *pointing at* an Array — so without
  # this the copy and the original share one table and `h.dup[:b] = 2` mutates
  # `h`. Silently, with the right-looking answer for every read.
  #
  # `Kernel#dup` and `Kernel#clone` both call this (#201).
  def initialize_copy(other)
    pairs = []
    other.each_pair { |key, value| pairs.push([key, value]) }
    @__pairs__ = pairs
    @__hashes__ = nil
    # Measured: `compare_by_identity` survives `dup` and `clone`.
    @__identity__ = other.compare_by_identity?
    __copy_default_from__(other)
    self
  end

  def default(*key)
    return @__default__ unless @__default_is_proc__
    key.empty? ? nil : @__default__.call(self, key[0])
  end

  def default=(value)
    __check_frozen__
    @__default__ = value
    @__default_is_proc__ = false
    value
  end

  def default_proc
    @__default_is_proc__ ? @__default__ : nil
  end

  # Two checks Ruby makes and the messages it makes them with, measured: a
  # non-Proc that cannot be converted is "wrong default_proc type X (expected
  # Proc)", and a *lambda* of the wrong arity is "default_proc takes two
  # arguments (2 for N)". A plain proc is not arity-checked, because a proc
  # ignores extra arguments anyway.
  def default_proc=(block)
    __check_frozen__
    if block.nil?
      @__default__ = nil
      @__default_is_proc__ = false
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
    @__default__ = block
    @__default_is_proc__ = true
    block
  end

  # Which of the two a hash carries, copied whole. `compact` and `replace` keep
  # the default; `Hash[]`, `except` and `slice` deliberately do not — measured,
  # and the reason those three build a fresh hash rather than `dup` one.
  def __copy_default_from__(other)
    @__default__ = other.__raw_default__
    @__default_is_proc__ = other.__default_is_proc__
    self
  end

  def __raw_default__
    @__default__
  end

  def __default_is_proc__
    @__default_is_proc__
  end

  def size
    @__pairs__.size
  end

  def length
    size
  end

  def empty?
    size == 0
  end

  # Key identity is `hash` then `eql?`, not `==`, which is CRuby's (#22): a
  # candidate must have the same hash code *and* be `eql?` — asked of the key
  # being looked up — or be the very same object. `1` and `1.0` stay distinct
  # keys because they are `==` but not `eql?`, and a key class whose `hash`
  # disagrees with its `eql?` is not found, as in Ruby. Every lookup here goes
  # through this one method, so `[]`, `[]=`, `key?`, `fetch` and `delete` all
  # agree by construction.
  #
  # A key's hash code is taken when it is stored, in `@__hashes__` beside
  # `@__pairs__`, so a key mutated afterwards is not found until `rehash` — which
  # is Ruby's behaviour and what `rehash_spec.rb` checks. `@__hashes__` is dropped
  # (nil) by anything that rebuilds `@__pairs__`, and recomputed on demand.
  #
  # Under `compare_by_identity` the only question is identity, and `hash` is
  # never called: measured.
  def __index__(key)
    pairs = @__pairs__
    i = 0
    if @__identity__
      id = key.__id__
      while i < pairs.size
        return i if pairs[i][0].__id__ == id
        i = i + 1
      end
      return nil
    end
    code = __element_hash__(key)
    hashes = __hashes__
    while i < pairs.size
      if hashes[i] == code
        stored = pairs[i][0]
        return i if stored.equal?(key) || key.eql?(stored)
      end
      i = i + 1
    end
    nil
  end

  def __hashes__
    @__hashes__ ||= @__pairs__.map { |pair| __element_hash__(pair[0]) }
  end

  def compare_by_identity
    raise FrozenError.new("can't modify frozen Hash: " + inspect, receiver: self) if frozen?
    @__identity__ = true
    @__hashes__ = nil
    self
  end

  def compare_by_identity?
    @__identity__ == true
  end

  # Every key's hash code taken afresh. Two keys that have become `eql?` since
  # they were stored collapse into the first one's position, keeping the later
  # value — CRuby re-inserts in order. Measured.
  def rehash
    __check_frozen__
    pairs = @__pairs__
    @__pairs__ = []
    @__hashes__ = []
    pairs.each { |pair| self[pair[0]] = pair[1] }
    self
  end

  # A miss goes through `default`, which is a method rather than the ivar: a
  # `Hash` subclass overriding `default(key)` is how ruby/spec's `DefaultHash`
  # answers 100 for every key, and reading `@__default__` here would never see it.
  def [](key)
    at = __index__(key)
    return @__pairs__[at][1] unless at.nil?
    default(key)
  end

  # `fetch(k, nil)` answers nil; only `fetch(k)` with no block raises. So this
  # counts the arguments given rather than testing one for nil.
  def fetch(*fallback)
    if fallback.size < 1 || fallback.size > 2
      raise ArgumentError,
            "wrong number of arguments (given " + fallback.size.to_s + ", expected 1..2)"
    end
    __warning__("block supersedes default value argument") if fallback.size > 1 && block_given?
    key = fallback[0]
    at = __index__(key)
    return @__pairs__[at][1] unless at.nil?
    return yield(key) if block_given?
    return fallback[1] if fallback.size > 1
    raise KeyError, "key not found: " + key.inspect
  end

  # A String key that is not frozen is stored as a frozen copy, so mutating
  # the caller's String cannot move it in the table — measured, and
  # `compare_by_identity` is the one table that keeps the caller's own object.
  def []=(key, value)
    __check_frozen__
    at = __index__(key)
    if at.nil?
      if !@__identity__ && key.is_a?(String) && !key.frozen?
        key = key.dup.freeze
      end
      @__pairs__.push([key, value])
      @__hashes__.push(__element_hash__(key)) unless @__hashes__.nil? || @__identity__
    else
      @__pairs__[at][1] = value
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

  # The implicit conversion: the receiver, a subclass's included. Measured.
  def to_hash = self

  # `to_h` hands the block the key and the value as two arguments, where
  # Enumerable's would hand it the one pair. Without a block it answers the
  # receiver itself, not a copy. Measured.
  def to_h
    # A subclass answers a plain Hash copy, default included; a Hash answers
    # itself. Measured.
    unless block_given?
      return self if instance_of?(Hash)
      out = __like_self__
      each_pair { |k, v| out[k] = v }
      out.__copy_default_from__(self)
      return out
    end
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
    return to_enum(:select) { size } unless block_given?
    out = __like_self__
    each_pair { |pair| out[pair[0]] = pair[1] if yield(pair[0], pair[1]) }
    out
  end

  def filter
    return to_enum(:filter) { size } unless block_given?
    out = __like_self__
    each_pair { |pair| out[pair[0]] = pair[1] if yield(pair[0], pair[1]) }
    out
  end

  def reject
    return to_enum(:reject) { size } unless block_given?
    out = __like_self__
    each_pair { |pair| out[pair[0]] = pair[1] unless yield(pair[0], pair[1]) }
    out
  end

  def keys
    @__pairs__.map { |pair| pair[0] }
  end

  def values
    @__pairs__.map { |pair| pair[1] }
  end

  # Yields one value, the `[key, value]` pair — not two. Measured: a
  # `{ |x| }` block over a hash binds `x` to the pair, and a `{ |k, v| }` one
  # gets its two locals from the block's own auto-splat rather than from here.
  # Yielding two would leave the first shape holding only the key.
  def each
    return to_enum(:each) { size } unless block_given?
    @__pairs__.each { |pair| yield pair }
    self
  end

  def each_pair
    return to_enum(:each_pair) { size } unless block_given?
    @__pairs__.each { |pair| yield pair }
    self
  end

  def each_key
    return to_enum(:each_key) { size } unless block_given?
    keys.each { |key| yield key }
    self
  end

  def each_value
    return to_enum(:each_value) { size } unless block_given?
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
    pairs = @__pairs__
    gone = pairs[at][1]
    kept = []
    i = 0
    while i < pairs.size
      kept.push(pairs[i]) unless i == at
      i = i + 1
    end
    @__pairs__ = kept
    @__hashes__ = nil
    gone
  end

  # A Hash is its own hash pattern subject (#165). The key list is ignored:
  # Ruby's own `Hash#deconstruct_keys` answers the whole hash either way, and
  # the pattern picks what it wants out of it.
  def deconstruct_keys(keys)
    self
  end

  def to_a
    @__pairs__.map { |pair| [pair[0], pair[1]] }
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
    # The argument's comparison comes over with its pairs, and the receiver's
    # own is dropped: measured both ways.
    @__identity__ = other.compare_by_identity?
    @__hashes__ = nil
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
    return value if path.size == 1 || value.nil?
    unless value.respond_to?(:dig)
      raise TypeError, value.class.name + " does not have #dig method"
    end
    # The rest of the path goes to the inner object in one call, which does
    # the rest itself: measured, an object's own `dig` is not recursed into.
    value.dig(*path.__take__(1, path.size - 1))
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
    out = __like_self__
    # The pair's value, read straight from the table: `__index__` is where the
    # pair is, and a subclass's own `[]` is not consulted — measured, and
    # `slice_spec.rb` pins it.
    wanted.each do |key|
      at = __index__(key)
      out[key] = @__pairs__[at][1] unless at.nil?
    end
    out
  end

  def except(*unwanted)
    out = __like_self__
    each_pair { |k, v| out[k] = v }
    unwanted.each { |key| out.delete(key) }
    out
  end

  # --- in place --------------------------------------------------------------

  # `reject!`, `select!` and `compact!` answer nil when they changed nothing,
  # while `delete_if` and `keep_if` always answer self. That is the only
  # difference between the two pairs, and it is measured rather than guessed.
  def reject!(&block)
    return to_enum(:reject!) { size } if block.nil?
    __check_frozen__
    removed = __remove_where__(block, true)
    removed ? self : nil
  end

  def delete_if(&block)
    return to_enum(:delete_if) { size } if block.nil?
    __check_frozen__
    __remove_where__(block, true)
    self
  end

  def select!(&block)
    return to_enum(:select!) { size } if block.nil?
    __check_frozen__
    removed = __remove_where__(block, false)
    removed ? self : nil
  end

  def filter!(&block)
    select!(&block)
  end

  def keep_if(&block)
    return to_enum(:keep_if) { size } if block.nil?
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
    out = __like_self__
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
    return to_enum(:transform_values) { size } if block.nil?
    out = __like_self__
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
    return to_enum(:transform_keys) { size } if block.nil? && mapping.empty?
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
    raise FrozenError.new("can't modify frozen Hash: " + inspect, receiver: self) if frozen?
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
  # Two empty hashes are equal whatever their comparison, and two non-empty
  # ones that differ only in `compare_by_identity` are not. Measured.
  #
  # A pair already being compared is taken as equal, which is how
  # `h = {}; h[:x] = h; h == {x: h}` terminates and answers true, as CRuby's
  # `rb_exec_recursive_paired` does. It shares the stack `Array#==` uses,
  # because the recursion can run through either.
  def __same_pairs__(other, strict)
    return false unless size == other.size
    return true if empty?
    return false unless compare_by_identity? == other.compare_by_identity?
    comparing = Array.__comparing__
    return true if comparing.any? { |a, b| a.equal?(self) && b.equal?(other) }
    comparing.push([self, other])
    begin
      each_pair do |key, value|
        return false unless other.key?(key)
        theirs = other[key]
        return false unless strict ? value.eql?(theirs) : value == theirs
      end
    ensure
      comparing.pop
    end
    true
  end

  def inspect
    return "{}".b.__force_encoding__(2) if empty?
    Kernel.__inspect_guard__(self, "{...}") do
      out = "{".b.__force_encoding__(2)
      pairs = @__pairs__
      i = 0
      while i < pairs.size
        out << ", " if i > 0
        key = pairs[i][0]
        if key.is_a?(Symbol)
          out << Hash.__symbol_key__(key) << " " << pairs[i][1].inspect
        else
          out << key.inspect << " => " << pairs[i][1].inspect
        end
        i += 1
      end
      out << "}"
    end
  end

  # Ruby 3.4's form for a Symbol key: `name:` when the name is a plain
  # identifier the default external encoding can show, and a quoted
  # `"name":` otherwise. Measured.
  def self.__symbol_key__(key)
    name = key.to_s
    plain = name.match?(/\A[A-Za-z_\u0080-\u{10FFFF}][A-Za-z0-9_\u0080-\u{10FFFF}]*[?!]?\z/) &&
            (name.ascii_only? || Encoding.default_external == name.encoding)
    plain ? "#{name}:" : "#{name.inspect}:"
  end

  alias to_s inspect

  # An empty Hash that compares keys the way this one does: `select`, `reject`,
  # `slice`, `except`, `compact` and `transform_values` keep
  # `compare_by_identity`, and `transform_keys` and `invert` drop it. Measured
  # on ruby 4.0.7, one method at a time.
  def __like_self__
    out = {}
    out.compare_by_identity if compare_by_identity?
    out
  end

  # A content digest (#22): the same whatever order the pairs were added in —
  # each pair's digest is summed — and the same for two hashes that are `eql?`,
  # a self-containing one included, through `__recursive_hash__`.
  def hash
    __recursive_hash__(__hash_combine__(:__spinel_recursive_hash__, size)) do
      sum = 0
      each_pair do |key, value|
        pair = __hash_combine__(__element_hash__(key), __element_hash__(value))
        sum = (sum + pair) & 0x3fffffffffffffff
      end
      __hash_combine__(:__spinel_hash__, size, sum)
    end
  end

  # The first pair, removed. Nil when there is none — measured, whatever the
  # default says: `Hash.new(5).shift` is nil.
  def shift
    __check_frozen__
    return nil if empty?
    pair = @__pairs__[0]
    delete(pair[0])
    [pair[0], pair[1]]
  end

  # The subset order: `a <= b` when every pair of `a` is in `b`, by the same
  # rules `==` uses — keys by the table, values by `==`.
  def <=(other)
    other = Hash.__convert__(other)
    return false if size > other.size
    each_pair do |key, value|
      return false unless other.key?(key) && other[key] == value
    end
    true
  end

  def <(other)
    other = Hash.__convert__(other)
    size < other.size && self <= other
  end

  def >=(other)
    Hash.__convert__(other) <= self
  end

  def >(other)
    Hash.__convert__(other) < self
  end

  # The first key whose value is `==` to `value`; never the default.
  def key(value)
    each_pair { |k, v| return k if v == value }
    nil
  end

  # In place, with CRuby's order: each original pair is removed — unless its
  # key is one this call already produced — and its new key stored, so a new
  # key never collides with an old one still waiting to be renamed. A Hash
  # argument maps what it holds and leaves the rest to the block.
  def transform_keys!(*mapping, &block)
    if mapping.empty? && block.nil?
      return to_enum(:transform_keys!) { size }
    end
    __check_frozen__
    table = mapping.empty? ? nil : mapping[0]
    if !mapping.empty? && table.nil?
      raise TypeError, "no implicit conversion of nil into Hash"
    end
    produced = {}
    to_a.each do |key, value|
      delete(key) unless produced.key?(key)
      new_key = if !table.nil? && table.key?(key)
        table[key]
      elsif !block.nil?
        block.call(key)
      else
        key
      end
      self[new_key] = value
      produced[new_key] = nil
    end
    self
  end

  # A Hash as is, anything with `to_hash` through it, and nil otherwise; a
  # `to_hash` that answers something else is a TypeError. Measured.
  def self.try_convert(value)
    return value if value.is_a?(Hash)
    return nil unless value.respond_to?(:to_hash)
    converted = value.to_hash
    return converted if converted.nil? || converted.is_a?(Hash)
    raise TypeError,
          "can't convert #{value.class} to Hash (#{value.class}#to_hash gives #{converted.class})"
  end

  def self.__convert__(value)
    return value if value.is_a?(Hash)
    raise TypeError, "no implicit conversion of #{value.class} into Hash" unless value.respond_to?(:to_hash)
    value.to_hash
  end
end
