# Enumerable — every method written against `each` alone.
#
# That constraint is the module's whole contract: a class supplies `each` and
# gets the other sixty. It is also what `core/enumerable/` actually tests, since
# its fixtures are classes whose only method is `each`.
#
# Two yield conventions, and they are not interchangeable. A block passed to
# `map` sees the values `each` yielded, with their original arity; a block
# passed to `select` sees them packed into one value. Measured on ruby 4.0.6 by
# `scripts/enumerable-oracle.rb`, which prints what the block received for each
# method — the table below is that output, not a reading of the docs.
#
#   pass-through   map flat_map filter_map find_index all? any? none? one?
#                  count take_while uniq to_h
#   packed         select reject find group_by partition sort_by min_by max_by
#                  each_with_object each_with_index drop_while inject sum
#                  each_entry reverse_each cycle chunk chunk_while slice_when
#
# A method with no block returns an Enumerator over itself, so `each_slice(2)`
# without a block is `to_enum(:each_slice, 2)` and the caller can chain.
module Enumerable
  # `yield 1, 2` reaches a collecting method as `[1, 2]`; `yield 6` as `6`.
  # This is CRuby's `rb_enum_values_pack`, and getting it wrong is the
  # difference between `[[1, 2], 6]` and `[1, 6]`.
  def __pack__(args)
    args.size <= 1 ? args[0] : args
  end

  # --- the collectors -------------------------------------------------------

  def to_a(*args)
    out = []
    each(*args) { |*a| out.push(__pack__(a)) }
    out
  end

  def entries(*args)
    to_a(*args)
  end

  def count(*item)
    if item.size > 1
      raise ArgumentError, "wrong number of arguments (given #{item.size}, expected 0..1)"
    end
    n = 0
    if item.size == 1
      wanted = item[0]
      each { |*a| n = n + 1 if __pack__(a) == wanted }
    elsif block_given?
      each { |*a| n = n + 1 if yield(*a) }
    else
      each { |*_a| n = n + 1 }
    end
    n
  end

  def first(*count)
    if count.size > 1
      raise ArgumentError, "wrong number of arguments (given #{count.size}, expected 0..1)"
    end
    if count.empty?
      each { |*a| return __pack__(a) }
      return nil
    end
    wanted = count[0]
    raise ArgumentError, "negative array size" if wanted < 0
    out = []
    return out if wanted == 0
    each do |*a|
      out.push(__pack__(a))
      return out if out.size >= wanted
    end
    out
  end

  # `wanted == item`, not `item == wanted`. This is CRuby's `rb_equal(arg, e)`,
  # and the direction is observable: an argument with a bespoke `==` has it
  # called, with the element as its operand.
  def include?(wanted)
    each { |*a| return true if wanted == __pack__(a) }
    false
  end

  def member?(wanted)
    each { |*a| return true if wanted == __pack__(a) }
    false
  end

  # `each_slice(3.3)` is legal: a non-Integer size is converted with `to_int`.
  def __to_int__(n)
    return n if n.is_a?(Integer)
    unless n.respond_to?(:to_int)
      raise TypeError, "no implicit conversion of #{n.class} into Integer"
    end
    converted = n.to_int
    unless converted.is_a?(Integer)
      raise TypeError, "can't convert #{n.class} to Integer"
    end
    converted
  end

  # An enumerable that answers `size` with an Integer knows its length without
  # iterating, which is what lets `each_slice` hand its enumerator a size.
  def __known_size__
    return nil unless respond_to?(:size)
    total = size
    total.is_a?(Integer) ? total : nil
  end

  # `lazy` starts a chain that defers every link until something forces it.
  # The root passes the yielded values through untouched, so a multi-value
  # `each` still reaches the first link with its original arity.
  def lazy
    Enumerator::Lazy.__link__(self, __known_size__, nil) do
      ->(y, *values) { y.yield(*values) }
    end
  end

  # `chain` reads several enumerables end to end without copying any of them.
  def chain(*others)
    Enumerator::Chain.new(self, *others)
  end

  # --- mapping --------------------------------------------------------------

  def map
    return to_enum(:map) unless block_given?
    out = []
    each { |*a| out.push(yield(*a)) }
    out
  end

  def collect
    return to_enum(:collect) unless block_given?
    out = []
    each { |*a| out.push(yield(*a)) }
    out
  end

  def flat_map
    return to_enum(:flat_map) unless block_given?
    out = []
    each do |*a|
      value = yield(*a)
      if value.is_a?(Array)
        value.each { |element| out.push(element) }
      else
        out.push(value)
      end
    end
    out
  end

  def collect_concat
    return to_enum(:collect_concat) unless block_given?
    out = []
    each do |*a|
      value = yield(*a)
      if value.is_a?(Array)
        value.each { |element| out.push(element) }
      else
        out.push(value)
      end
    end
    out
  end

  def filter_map
    return to_enum(:filter_map) unless block_given?
    out = []
    each do |*a|
      value = yield(*a)
      out.push(value) if value
    end
    out
  end

  # --- filtering ------------------------------------------------------------

  def select
    return to_enum(:select) unless block_given?
    out = []
    each { |*a| item = __pack__(a); out.push(item) if yield(item) }
    out
  end

  def filter
    return to_enum(:filter) unless block_given?
    out = []
    each { |*a| item = __pack__(a); out.push(item) if yield(item) }
    out
  end

  def find_all
    return to_enum(:find_all) unless block_given?
    out = []
    each { |*a| item = __pack__(a); out.push(item) if yield(item) }
    out
  end

  def reject
    return to_enum(:reject) unless block_given?
    out = []
    each { |*a| item = __pack__(a); out.push(item) unless yield(item) }
    out
  end

  # `find(ifnone)` calls `ifnone` only when nothing matched, and does not check
  # up front that it is callable — so a non-callable raises NoMethodError at the
  # point it would have been called.
  def find(ifnone = nil)
    return to_enum(:find, ifnone) unless block_given?
    each { |*a| item = __pack__(a); return item if yield(item) }
    ifnone.nil? ? nil : ifnone.call
  end

  def detect(ifnone = nil)
    return to_enum(:detect, ifnone) unless block_given?
    each { |*a| item = __pack__(a); return item if yield(item) }
    ifnone.nil? ? nil : ifnone.call
  end

  def find_index(*wanted)
    if wanted.size > 1
      raise ArgumentError, "wrong number of arguments (given #{wanted.size}, expected 0..1)"
    end
    return to_enum(:find_index) if wanted.empty? && !block_given?
    i = 0
    if wanted.size == 1
      target = wanted[0]
      each do |*a|
        return i if __pack__(a) == target
        i = i + 1
      end
    else
      each do |*a|
        return i if yield(*a)
        i = i + 1
      end
    end
    nil
  end

  def grep(pattern)
    out = []
    each do |*a|
      item = __pack__(a)
      next unless pattern === item
      out.push(block_given? ? yield(item) : item)
    end
    out
  end

  def grep_v(pattern)
    out = []
    each do |*a|
      item = __pack__(a)
      next if pattern === item
      out.push(block_given? ? yield(item) : item)
    end
    out
  end

  def compact
    out = []
    each { |*a| item = __pack__(a); out.push(item) unless item.nil? }
    out
  end

  # Keyed by a Hash, so `eql?`/`hash` decide sameness rather than `==`. The
  # difference is observable: `[1.0, 1].uniq` keeps both, because `1.0.eql?(1)`
  # is false while `1.0 == 1` is true.
  def uniq
    out = []
    seen = {}
    each do |*a|
      item = __pack__(a)
      key = block_given? ? yield(*a) : item
      next if seen.key?(key)
      seen[key] = true
      out.push(item)
    end
    out
  end

  # --- predicates -----------------------------------------------------------

  def all?(*pattern)
    if pattern.size > 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 0..1)"
    end
    if pattern.size == 1
      matcher = pattern[0]
      each { |*a| return false unless matcher === __pack__(a) }
    elsif block_given?
      each { |*a| return false unless yield(*a) }
    else
      each { |*a| return false unless __pack__(a) }
    end
    true
  end

  def any?(*pattern)
    if pattern.size > 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 0..1)"
    end
    if pattern.size == 1
      matcher = pattern[0]
      each { |*a| return true if matcher === __pack__(a) }
    elsif block_given?
      each { |*a| return true if yield(*a) }
    else
      each { |*a| return true if __pack__(a) }
    end
    false
  end

  def none?(*pattern)
    if pattern.size > 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 0..1)"
    end
    if pattern.size == 1
      matcher = pattern[0]
      each { |*a| return false if matcher === __pack__(a) }
    elsif block_given?
      each { |*a| return false if yield(*a) }
    else
      each { |*a| return false if __pack__(a) }
    end
    true
  end

  def one?(*pattern)
    if pattern.size > 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 0..1)"
    end
    seen = 0
    if pattern.size == 1
      matcher = pattern[0]
      each { |*a| seen = seen + 1 if matcher === __pack__(a); return false if seen > 1 }
    elsif block_given?
      each { |*a| seen = seen + 1 if yield(*a); return false if seen > 1 }
    else
      each { |*a| seen = seen + 1 if __pack__(a); return false if seen > 1 }
    end
    seen == 1
  end

  # --- folding --------------------------------------------------------------

  # Four call shapes: `inject { }`, `inject(init) { }`, `inject(:sym)`, and
  # `inject(init, :sym)`. The symbol forms take no block.
  def inject(*given)
    if given.size > 2
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..2)"
    end
    if given.empty? && !block_given?
      raise ArgumentError, "wrong number of arguments (given 0, expected 1..2)"
    end
    operation = nil
    accumulator = nil
    started = false
    if given.size == 2
      accumulator = given[0]
      started = true
      operation = given[1]
    elsif given.size == 1
      if block_given?
        accumulator = given[0]
        started = true
      else
        operation = given[0]
      end
    end
    each do |*a|
      item = __pack__(a)
      if !started
        accumulator = item
        started = true
      elsif operation.nil?
        accumulator = yield(accumulator, item)
      else
        accumulator = accumulator.send(operation, item)
      end
    end
    accumulator
  end

  def reduce(*given)
    if block_given?
      inject(*given) { |acc, item| yield(acc, item) }
    else
      inject(*given)
    end
  end

  def sum(initial = 0)
    total = initial
    if block_given?
      each { |*a| total = total + yield(__pack__(a)) }
    else
      each { |*a| total = total + __pack__(a) }
    end
    total
  end

  # --- grouping -------------------------------------------------------------

  def group_by
    return to_enum(:group_by) unless block_given?
    out = {}
    each do |*a|
      item = __pack__(a)
      key = yield(item)
      bucket = out[key]
      if bucket.nil?
        out[key] = [item]
      else
        bucket.push(item)
      end
    end
    out
  end

  def partition
    return to_enum(:partition) unless block_given?
    yes = []
    no = []
    each do |*a|
      item = __pack__(a)
      if yield(item)
        yes.push(item)
      else
        no.push(item)
      end
    end
    [yes, no]
  end

  # `tally(hash)` counts into the hash it was given and returns that same hash,
  # so an existing count is added to rather than replaced.
  def tally(*given)
    if given.size > 1
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..1)"
    end
    out = {}
    if given.size == 1
      out = given[0]
      unless out.is_a?(Hash)
        unless out.respond_to?(:to_hash)
          raise TypeError, "no implicit conversion of #{out.class} into Hash"
        end
        out = out.to_hash
      end
    end
    each do |*a|
      item = __pack__(a)
      seen = out[item]
      out[item] = seen.nil? ? 1 : seen + 1
    end
    out
  end

  def to_h(*args)
    out = {}
    each(*args) do |*a|
      pair = block_given? ? yield(*a) : __pack__(a)
      unless pair.is_a?(Array)
        raise TypeError, "wrong element type #{pair.class} (expected array)"
      end
      unless pair.size == 2
        raise ArgumentError, "element has wrong array length (expected 2, was #{pair.size})"
      end
      out[pair[0]] = pair[1]
    end
    out
  end

  # --- ordering -------------------------------------------------------------

  # Merge sort: O(n log n) with no quadratic worst case, and short enough to
  # read. Ruby's own sort is an unstable quicksort, so a program may not rely on
  # order among equal elements either way.
  #
  # ponytail: a Ruby-level merge sort allocates one array per merge. If sorting
  # shows up in a benchmark, the upgrade path is a primitive on the Array
  # storage, not a cleverer algorithm here.
  def sort(&block)
    __merge_sort__(to_a, block)
  end

  def sort_by
    return to_enum(:sort_by) unless block_given?
    keyed = []
    each { |*a| item = __pack__(a); keyed.push([yield(item), item]) }
    sorted = __merge_sort__(keyed, proc { |x, y| x[0] <=> y[0] })
    out = []
    sorted.each { |pair| out.push(pair[1]) }
    out
  end

  def __merge_sort__(list, comparator)
    return list if list.size <= 1
    middle = list.size / 2
    left = __merge_sort__(list.__take__(0, middle), comparator)
    right = __merge_sort__(list.__take__(middle, list.size - middle), comparator)
    out = []
    i = 0
    j = 0
    while i < left.size && j < right.size
      a = left[i]
      b = right[j]
      cmp = comparator.nil? ? (a <=> b) : comparator.call(a, b)
      if cmp.nil?
        raise ArgumentError, "comparison of #{a.class} with #{b.class} failed"
      end
      if cmp <= 0
        out.push(a)
        i = i + 1
      else
        out.push(b)
        j = j + 1
      end
    end
    while i < left.size
      out.push(left[i])
      i = i + 1
    end
    while j < right.size
      out.push(right[j])
      j = j + 1
    end
    out
  end

  def min(*count, &block)
    __extreme__(count, block, -1)
  end

  def max(*count, &block)
    __extreme__(count, block, 1)
  end

  # `min` and `max` differ only in the sign they keep, and `min(n)`/`max(n)`
  # only in which end of a sorted list they take.
  def __extreme__(count, block, want)
    if count.size > 1
      raise ArgumentError, "wrong number of arguments (given #{count.size}, expected 0..1)"
    end
    if count.size == 1 && !count[0].nil?
      n = count[0]
      raise ArgumentError, "negative size (#{n})" if n < 0
      sorted = __merge_sort__(to_a, block)
      sorted = sorted.reverse if want > 0
      return sorted.__take__(0, n)
    end
    best = nil
    seen = false
    each do |*a|
      item = __pack__(a)
      if !seen
        best = item
        seen = true
        next
      end
      cmp = block.nil? ? (item <=> best) : block.call(item, best)
      if cmp.nil?
        raise ArgumentError, "comparison of #{item.class} with #{best.class} failed"
      end
      best = item if (want > 0 && cmp > 0) || (want < 0 && cmp < 0)
    end
    best
  end

  def min_by(*count)
    return to_enum(:min_by) unless block_given?
    __extreme_by__(count, -1) { |item| yield(item) }
  end

  def max_by(*count)
    return to_enum(:max_by) unless block_given?
    __extreme_by__(count, 1) { |item| yield(item) }
  end

  def __extreme_by__(count, want)
    if count.size == 1 && !count[0].nil?
      count[0] = __to_int__(count[0])
      raise ArgumentError, "negative size (#{count[0]})" if count[0] < 0
    end
    keyed = []
    each { |*a| item = __pack__(a); keyed.push([yield(item), item]) }
    sorted = __merge_sort__(keyed, proc { |x, y| x[0] <=> y[0] })
    sorted = sorted.reverse if want > 0
    if count.size == 1 && !count[0].nil?
      out = []
      sorted.__take__(0, count[0]).each { |pair| out.push(pair[1]) }
      return out
    end
    return nil if sorted.empty?
    sorted[0][1]
  end

  def minmax(&block)
    [min(&block), max(&block)]
  end

  def minmax_by(&block)
    return to_enum(:minmax_by) if block.nil?
    [min_by { |item| block.call(item) }, max_by { |item| block.call(item) }]
  end

  # --- slicing --------------------------------------------------------------

  def take(n)
    n = __to_int__(n)
    raise ArgumentError, "attempt to take negative size" if n < 0
    out = []
    return out if n == 0
    each do |*a|
      out.push(__pack__(a))
      return out if out.size >= n
    end
    out
  end

  def take_while
    return to_enum(:take_while) unless block_given?
    out = []
    each do |*a|
      return out unless yield(*a)
      out.push(__pack__(a))
    end
    out
  end

  def drop(n)
    n = __to_int__(n)
    raise ArgumentError, "attempt to drop negative size" if n < 0
    out = []
    i = 0
    each do |*a|
      out.push(__pack__(a)) if i >= n
      i = i + 1
    end
    out
  end

  def drop_while
    return to_enum(:drop_while) unless block_given?
    out = []
    dropping = true
    each do |*a|
      item = __pack__(a)
      dropping = false if dropping && !yield(item)
      out.push(item) unless dropping
    end
    out
  end

  def each_slice(n)
    n = __to_int__(n)
    raise ArgumentError, "invalid slice size" if n <= 0
    unless block_given?
      total = __known_size__
      slices = total.nil? ? nil : (total + n - 1) / n
      return Enumerator.__for__(self, :each_slice, [n], slices)
    end
    slice = []
    each do |*a|
      slice.push(__pack__(a))
      if slice.size == n
        yield slice
        slice = []
      end
    end
    yield slice unless slice.empty?
    self
  end

  def each_cons(n)
    n = __to_int__(n)
    raise ArgumentError, "invalid size" if n <= 0
    unless block_given?
      total = __known_size__
      windows = nil
      unless total.nil?
        windows = total - n + 1
        windows = 0 if windows < 0
      end
      return Enumerator.__for__(self, :each_cons, [n], windows)
    end
    window = []
    each do |*a|
      window.push(__pack__(a))
      window.shift if window.size > n
      yield window.dup if window.size == n
    end
    self
  end

  def each_with_index(*args)
    return to_enum(:each_with_index, *args) unless block_given?
    i = 0
    each(*args) do |*a|
      yield __pack__(a), i
      i = i + 1
    end
    self
  end

  def each_with_object(memo)
    return to_enum(:each_with_object, memo) unless block_given?
    each { |*a| yield __pack__(a), memo }
    memo
  end

  def each_entry(*args)
    return to_enum(:each_entry, *args) unless block_given?
    each(*args) { |*a| yield __pack__(a) }
    self
  end

  def reverse_each(*args)
    unless block_given?
      return Enumerator.__for__(self, :reverse_each, args, __known_size__)
    end
    to_a(*args).reverse.each { |item| yield item }
    self
  end

  # `cycle` with no count repeats forever, which is a legal thing to ask for:
  # the block is expected to break out.
  def cycle(count = nil)
    return to_enum(:cycle, count) unless block_given?
    unless count.nil?
      count = __to_int__(count)
      return nil if count <= 0
    end
    # The first pass drives `each` directly and fills the cache as it goes, so a
    # block that breaks part way through leaves the rest of the enumerable
    # un-iterated. Building the cache with `to_a` first would yield to `each`
    # for every element before the block saw any of them, which the spec
    # measures by counting yields.
    cached = []
    each do |*a|
      item = __pack__(a)
      cached.push(item)
      yield item
    end
    return nil if cached.empty?
    if count.nil?
      while true
        cached.each { |item| yield item }
      end
    else
      i = 1
      while i < count
        cached.each { |item| yield item }
        i = i + 1
      end
    end
    nil
  end

  def zip(*others)
    lists = []
    others.each do |other|
      if other.is_a?(Array)
        lists.push(other)
      elsif other.respond_to?(:to_ary)
        lists.push(other.to_ary)
      else
        lists.push(other.to_a)
      end
    end
    out = []
    i = 0
    each do |*a|
      row = [__pack__(a)]
      lists.each { |list| row.push(list[i]) }
      if block_given?
        yield row
      else
        out.push(row)
      end
      i = i + 1
    end
    block_given? ? nil : out
  end

  # --- run-splitting --------------------------------------------------------
  #
  # All five return an Enumerator, not an Array, so `size` answers nil and the
  # run is recomputed on each `each`. Measured on ruby 4.0.6; returning an Array
  # is invisible to `.to_a` and wrong for everything else.

  def chunk_while(*given)
    unless given.empty?
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0)"
    end
    unless block_given?
      raise ArgumentError, "tried to create Proc object without a block"
    end
    Enumerator.new do |collector|
      run = []
      previous = nil
      each do |*a|
        item = __pack__(a)
        if run.empty?
          run = [item]
        elsif yield(previous, item)
          run.push(item)
        else
          collector << run
          run = [item]
        end
        previous = item
      end
      collector << run unless run.empty?
    end
  end

  def slice_when(*given)
    unless given.empty?
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0)"
    end
    unless block_given?
      raise ArgumentError, "tried to create Proc object without a block"
    end
    Enumerator.new do |collector|
      run = []
      previous = nil
      each do |*a|
        item = __pack__(a)
        if run.empty?
          run = [item]
        elsif yield(previous, item)
          collector << run
          run = [item]
        else
          run.push(item)
        end
        previous = item
      end
      collector << run unless run.empty?
    end
  end

  # Three reserved keys, measured: `nil` and `:_separator` drop the element and
  # break the run, `:_alone` puts every element in a chunk of its own, and any
  # other symbol beginning with an underscore is an error rather than a key.
  def chunk(*given)
    unless given.empty?
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0)"
    end
    unless block_given?
      raise ArgumentError, "tried to create Proc object without a block"
    end
    Enumerator.new do |collector|
      run = nil
      key = nil
      each do |*a|
        item = __pack__(a)
        this_key = yield(item)
        __check_chunk_key__(this_key)
        if this_key.nil? || this_key == :_separator
          collector << [key, run] unless run.nil?
          run = nil
          key = nil
        elsif this_key == :_alone
          collector << [key, run] unless run.nil?
          collector << [:_alone, [item]]
          run = nil
          key = nil
        elsif run.nil?
          key = this_key
          run = [item]
        elsif this_key == key
          run.push(item)
        else
          collector << [key, run]
          key = this_key
          run = [item]
        end
      end
      collector << [key, run] unless run.nil?
    end
  end

  def __check_chunk_key__(key)
    return unless key.is_a?(Symbol)
    return if key == :_alone || key == :_separator
    name = key.to_s
    return unless name.start_with?("_")
    raise RuntimeError, "symbols beginning with an underscore are reserved"
  end

  def slice_before(*pattern)
    if block_given?
      unless pattern.empty?
        raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 0)"
      end
    elsif pattern.size != 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 1)"
    end
    matcher = pattern.empty? ? nil : pattern[0]
    Enumerator.new do |collector|
      run = []
      each do |*a|
        item = __pack__(a)
        starts = pattern.empty? ? yield(item) : (matcher === item)
        if starts && !run.empty?
          collector << run
          run = []
        end
        run.push(item)
      end
      collector << run unless run.empty?
    end
  end

  def slice_after(*pattern)
    if block_given?
      raise ArgumentError, "both pattern and block are given" unless pattern.empty?
    elsif pattern.size != 1
      raise ArgumentError, "wrong number of arguments (given #{pattern.size}, expected 1)"
    end
    matcher = pattern.empty? ? nil : pattern[0]
    Enumerator.new do |collector|
      run = []
      each do |*a|
        item = __pack__(a)
        run.push(item)
        ends = pattern.empty? ? yield(item) : (matcher === item)
        if ends
          collector << run
          run = []
        end
      end
      collector << run unless run.empty?
    end
  end
end
