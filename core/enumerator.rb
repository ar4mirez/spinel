# Enumerator — a call remembered so it can be made later.
#
# `to_enum(:each_slice, 2)` is the whole idea: every Enumerable method called
# without a block returns one of these, and performs the work when someone
# finally supplies one.
#
# There is one internal shape, the generator block, and the receiver/method form
# is built on top of it — `__for__` writes a block that sends the method and
# forwards each yield. Ruby 4.0 removed the public `Enumerator.new(obj, meth)`
# constructor, so `new` requires a block and `__for__` is what `to_enum` calls.
#
# ponytail: internal iteration only. `next`, `peek` and `rewind` hand a value
# back to the *caller* mid-iteration, which needs the producing call to suspend
# with its stack intact — fibers, #16. They raise NotImplementedError naming
# that issue rather than being faked with a `to_a` buffer, because a buffer
# answers `next` correctly and `Enumerator.new { loop { y << rand } }.next`
# wrongly, and a wrong answer is worse than a refusal.
class Enumerator
  include Enumerable

  # The one optional positional is the size: an Integer, something callable
  # that answers one, or nil for "not known without iterating".
  def initialize(*given, &block)
    if block.nil?
      raise ArgumentError, "tried to create Proc object without a block"
    end
    @producer = block
    @size = given.empty? ? nil : given[0]
    @object = nil
    @method = nil
    @arguments = nil
  end

  # `to_enum`'s form. Built with `allocate` rather than `new` because Ruby 4.0
  # removed the public receiver/method constructor: `Enumerator.new(1, :upto, 3)`
  # has to raise, and this has to not.
  def self.__for__(object, method, arguments, size)
    allocate.__init_for__(object, method, arguments, size)
  end

  def __init_for__(object, method, arguments, size)
    @producer = nil
    @object = object
    @method = method
    @arguments = arguments
    @size = size
    self
  end

  # With no block this is the identity, which is what lets `enum.each_slice(2)`
  # work: Enumerable calls `each`, gets an Enumerator back, and chains.
  #
  # Two paths, because they are two different things. A receiver/method
  # enumerator calls the method with the driving block itself, so the block's
  # value reaches the method and the method's value comes back —
  # `[1, 2, 3].select.each_with_index { false }` is `[]` only because `select`
  # saw that false. Routing it through a Yielder would swallow both, since
  # `Yielder#yield` answers nil by definition.
  def each(&block)
    return self if block.nil?
    return @object.send(@method, *@arguments, &block) if @producer.nil?
    @producer.call(Yielder.new(&block))
  end

  def with_index(offset = 0)
    return Enumerator.__for__(self, :with_index, [offset], size) unless block_given?
    i = offset
    # The block's value is the value the driven method sees, and `each` answers
    # what that method answered: `[1, 2, 3].select.each_with_index { false }` is
    # `[]` because `select` got the false and returned its own empty result.
    each do |*a|
      answer = yield(a.size <= 1 ? a[0] : a, i)
      i = i + 1
      answer
    end
  end

  def each_with_index(&block)
    return Enumerator.__for__(self, :each_with_index, [], size) if block.nil?
    with_index(0, &block)
  end

  def with_object(memo)
    return Enumerator.__for__(self, :with_object, [memo], size) unless block_given?
    each { |*a| yield(a.size <= 1 ? a[0] : a, memo) }
    memo
  end

  def each_with_object(memo, &block)
    return Enumerator.__for__(self, :each_with_object, [memo], size) if block.nil?
    with_object(memo, &block)
  end

  # The size this enumerator was given, calling it if it is callable.
  #
  # ponytail: only the sizes a caller hands over are known. Ruby computes them
  # for its own enumerators in more places than `each_slice` and `each_cons` do
  # here; `nil` is a legal answer for any enumerator, so the rest are incomplete
  # rather than wrong.
  def size
    return nil if @size.nil?
    @size.respond_to?(:call) ? @size.call : @size
  end

  def inspect
    return "#<Enumerator: uninitialized>" if @object.nil? && @producer.nil?
    if @object.nil?
      "#<Enumerator: #<Enumerator::Generator>:each>"
    elsif @arguments.nil? || @arguments.empty?
      "#<Enumerator: #{@object.inspect}:#{@method}>"
    else
      "#<Enumerator: #{@object.inspect}:#{@method}(#{__argument_list__})>"
    end
  end

  def __argument_list__
    parts = []
    @arguments.each { |argument| parts.push(argument.inspect) }
    parts.join(", ")
  end

  def to_s
    inspect
  end

  def next
    __needs_fibers__("next")
  end

  def peek
    __needs_fibers__("peek")
  end

  def rewind
    __needs_fibers__("rewind")
  end

  def next_values
    __needs_fibers__("next_values")
  end

  def peek_values
    __needs_fibers__("peek_values")
  end

  def __needs_fibers__(name)
    raise NotImplementedError,
          "Enumerator##{name} suspends the producing call, which needs fibers (#16)"
  end

  # `Enumerator::Lazy` — the chain that does not run until something forces it.
  #
  # One shape underneath: a lazy holds its source and a `@setup` proc. `each`
  # calls `@setup` once per run to build that run's transform, so a chain is
  # re-enumerable — `l.force` twice answers the same thing — and hands it a
  # fresh throw tag, which is how `take` stops a source that has no end.
  #
  # The yield convention is *not* `Enumerable`'s. Two methods disagree with
  # their eager twin, in opposite directions, and `scripts/lazy-oracle.rb` is
  # the measurement rather than this comment:
  #
  #   pass through   map collect take_while drop_while flat_map collect_concat
  #                  filter_map
  #   pack into one  select filter reject uniq grep grep_v
  #
  # `drop_while` packs eagerly and passes through lazily; `uniq` does the
  # reverse. Every fixture that yields one value per iteration hides both.
  class Lazy < Enumerator
    # The public constructor: the block is a link, called with the yielder and
    # whatever the source yielded, and writes onward with `y << v` or `y.yield`.
    def initialize(object, size = nil, &block)
      raise ArgumentError, "tried to call lazy new without a block" if block.nil?
      __init_link__(object, size, nil) { block }
    end

    # `setup` is called once per `each` and answers that run's link. It takes
    # the run's throw tag, which only the bounded links (`take`) use.
    def self.__link__(source, size, description, &setup)
      allocate.__init_link__(source, size, description, &setup)
    end

    def __init_link__(source, size, description, &setup)
      @source = source
      @size = size
      @description = description
      @setup = setup
      @object = nil
      @method = nil
      @arguments = nil
      @producer = nil
      self
    end

    # Drives the source through this run's link. The `catch` is what lets a
    # link stop iteration from arbitrarily deep inside a nested chain, which a
    # `return` cannot do: it would leave the link, not the enumeration.
    def each(&block)
      raise ArgumentError, "uninitialized enumerator" if @setup.nil?
      return self if block.nil?
      tag = Object.new
      link = @setup.call(tag)
      yielder = Yielder.new(&block)
      catch(tag) { @source.each { |*values| link.call(yielder, *values) } }
      self
    end

    def size
      raise ArgumentError, "uninitialized enumerator" if @setup.nil?
      @size
    end

    def lazy
      self
    end

    # `eager` is the one link that leaves the chain: an ordinary Enumerator
    # over the same elements, so `map` on it answers an Array again.
    def eager
      Enumerator.__for__(self, :each, [], @size)
    end

    # `to_enum` on a lazy stays lazy — that is the whole point of `eager`
    # existing as the way out. A named method is honoured rather than ignored:
    # `(1..6).lazy.to_enum(:each_slice, 2).first(2)` is `[[1, 2], [3, 4]]`, so
    # the link drives that method and not `each`.
    def to_enum(method = :each, *arguments)
      source = self
      size = method == :each && arguments.empty? ? @size : nil
      Lazy.__link__(Enumerator.__for__(source, method, arguments, size), size, nil) do
        ->(y, *values) { y.yield(*values) }
      end
    end

    def enum_for(method = :each, *arguments)
      to_enum(method, *arguments)
    end

    def force(*arguments)
      to_a(*arguments)
    end

    # `first` with a count is the reason the chain exists: it must stop the
    # source, not filter a materialised Array.
    def first(*count)
      return super if count.empty?
      wanted = __to_int__(count[0])
      raise ArgumentError, "attempt to take negative size" if wanted < 0
      out = []
      return out if wanted == 0
      each do |*values|
        out.push(__pack__(values))
        return out if out.size >= wanted
      end
      out
    end

    # --- links that pass the yielded values through ---------------------------

    def map(&block)
      raise ArgumentError, "tried to call lazy map without a block" if block.nil?
      __step__(@size, "map") { ->(y, *values) { y.yield(block.call(*values)) } }
    end

    def collect(&block)
      raise ArgumentError, "tried to call lazy collect without a block" if block.nil?
      __step__(@size, "collect") { ->(y, *values) { y.yield(block.call(*values)) } }
    end

    def flat_map(&block)
      raise ArgumentError, "tried to call lazy flat_map without a block" if block.nil?
      __step__(nil, "flat_map") { ->(y, *values) { __spread__(y, block.call(*values)) } }
    end

    def collect_concat(&block)
      raise ArgumentError, "tried to call lazy collect_concat without a block" if block.nil?
      __step__(nil, "collect_concat") { ->(y, *values) { __spread__(y, block.call(*values)) } }
    end

    def __spread__(yielder, value)
      if value.is_a?(Array)
        value.each { |element| yielder.yield(element) }
      else
        yielder.yield(value)
      end
    end

    def filter_map(&block)
      raise ArgumentError, "tried to call lazy filter_map without a block" if block.nil?
      __step__(nil, "filter_map") do
        ->(y, *values) do
          answer = block.call(*values)
          y.yield(answer) if answer
        end
      end
    end

    def take_while(&block)
      raise ArgumentError, "tried to call lazy take_while without a block" if block.nil?
      __bounded__(nil, "take_while") do |tag|
        ->(y, *values) do
          throw(tag) unless block.call(*values)
          y.yield(*values)
        end
      end
    end

    def drop_while(&block)
      raise ArgumentError, "tried to call lazy drop_while without a block" if block.nil?
      __step__(nil, "drop_while") do
        dropping = true
        ->(y, *values) do
          dropping = false if dropping && !block.call(*values)
          y.yield(*values) unless dropping
        end
      end
    end

    # --- links that pack the yielded values into one --------------------------

    def select(&block)
      raise ArgumentError, "tried to call lazy select without a block" if block.nil?
      __filter__(block, "select", true)
    end

    def filter(&block)
      raise ArgumentError, "tried to call lazy filter without a block" if block.nil?
      __filter__(block, "filter", true)
    end

    def find_all(&block)
      raise ArgumentError, "tried to call lazy find_all without a block" if block.nil?
      __filter__(block, "find_all", true)
    end

    def reject(&block)
      raise ArgumentError, "tried to call lazy reject without a block" if block.nil?
      __filter__(block, "reject", false)
    end

    def __filter__(block, description, keep)
      __step__(nil, description) do
        ->(y, *values) do
          item = __pack__(values)
          y.yield(item) if block.call(item) ? keep : !keep
        end
      end
    end

    def grep(pattern, &block)
      __step__(nil, "grep") do
        ->(y, *values) do
          item = __pack__(values)
          next unless pattern === item
          y.yield(block.nil? ? item : block.call(item))
        end
      end
    end

    def grep_v(pattern, &block)
      __step__(nil, "grep_v") do
        ->(y, *values) do
          item = __pack__(values)
          next if pattern === item
          y.yield(block.nil? ? item : block.call(item))
        end
      end
    end

    def uniq(&block)
      __step__(nil, "uniq") do
        seen = {}
        ->(y, *values) do
          item = __pack__(values)
          key = block.nil? ? item : block.call(item)
          next if seen.key?(key)
          seen[key] = true
          y.yield(item)
        end
      end
    end

    def compact
      __step__(nil, "compact") do
        ->(y, *values) do
          item = __pack__(values)
          y.yield(item) unless item.nil?
        end
      end
    end

    # --- links that count ------------------------------------------------------

    # `take(0)` must not reach the source at all: with an endless generator the
    # difference is observable, and `force` has to answer `[]` with the
    # producing block never entered.
    def take(n)
      wanted = __to_int__(n)
      raise ArgumentError, "attempt to take negative size" if wanted < 0
      return Lazy.__link__([], 0, "take(0)") { ->(y, *values) { y.yield(*values) } } if wanted == 0
      __bounded__(__take_size__(wanted), "take(#{wanted})") do |tag|
        taken = 0
        ->(y, *values) do
          y.yield(*values)
          taken = taken + 1
          throw(tag) if taken >= wanted
        end
      end
    end

    def __take_size__(wanted)
      return nil if @size.nil?
      @size < wanted ? @size : wanted
    end

    def drop(n)
      dropped_count = __to_int__(n)
      raise ArgumentError, "attempt to drop negative size" if dropped_count < 0
      __step__(__drop_size__(dropped_count), "drop(#{dropped_count})") do
        seen = 0
        ->(y, *values) do
          seen = seen + 1
          y.yield(*values) if seen > dropped_count
        end
      end
    end

    def __drop_size__(dropped_count)
      return nil if @size.nil?
      @size > dropped_count ? @size - dropped_count : 0
    end

    # With a block, the block runs for its side effect and its value is
    # *discarded* — the element passes through unchanged. Measured, not
    # guessed: `(10..13).lazy.with_index(1) { "X" }.first(3)` is `[10, 11, 12]`
    # in Ruby, where mapping the block's value would answer `["X", "X", "X"]`.
    # Without a block the element becomes the `[item, index]` pair instead.
    def with_index(offset = 0, &block)
      start = __to_int__(offset)
      __step__(@size, "with_index(#{start})") do
        at = start
        ->(y, *values) do
          item = __pack__(values)
          if block.nil?
            y.yield([item, at])
          else
            block.call(item, at)
            y.yield(item)
          end
          at = at + 1
        end
      end
    end

    def each_with_index(&block)
      with_index(0, &block)
    end

    def zip(*others)
      return super if others.any? { |other| !other.is_a?(Array) }
      __step__(@size, "zip") do
        at = 0
        ->(y, *values) do
          row = [__pack__(values)]
          others.each { |other| row.push(other[at]) }
          at = at + 1
          y.yield(row)
        end
      end
    end

    # --- plumbing --------------------------------------------------------------

    def __step__(size, description, &setup)
      Lazy.__link__(self, size, description) { |_tag| setup.call }
    end

    def __bounded__(size, description, &setup)
      Lazy.__link__(self, size, description) { |tag| setup.call(tag) }
    end

    def inspect
      return "#<Enumerator::Lazy: uninitialized>" if @setup.nil?
      body = @source.inspect
      body = "#{body}:#{@description}" unless @description.nil?
      "#<Enumerator::Lazy: #{body}>"
    end

    def to_s
      inspect
    end
  end

  # `Enumerator::Chain` — several enumerables read end to end as one.
  class Chain < Enumerator
    def initialize(*sources)
      @sources = sources
      @object = nil
      @method = nil
      @arguments = nil
      @producer = nil
      @size = nil
    end

    def each(&block)
      raise ArgumentError, "uninitialized chain" if @sources.nil?
      return to_enum(:each) if block.nil?
      @sources.each { |source| source.each { |*values| block.call(*values) } }
      self
    end

    # `nil` the moment one link does not know its own length, because the sum
    # of a known and an unknown is not a number.
    def size
      raise ArgumentError, "uninitialized chain" if @sources.nil?
      total = 0
      @sources.each do |source|
        return nil unless source.respond_to?(:size)
        part = source.size
        return nil unless part.is_a?(Integer)
        total = total + part
      end
      total
    end

    def rewind
      __needs_fibers__("rewind")
    end

    def inspect
      return "#<Enumerator::Chain: uninitialized>" if @sources.nil?
      "#<Enumerator::Chain: #{@sources.inspect}>"
    end

    def to_s
      inspect
    end
  end

  # `Enumerator::Product` — every combination, last argument varying fastest.
  class Product < Enumerator
    def initialize(*sources)
      @sources = sources
      @object = nil
      @method = nil
      @arguments = nil
      @producer = nil
      @size = nil
    end

    def each(&block)
      raise ArgumentError, "uninitialized product" if @sources.nil?
      return to_enum(:each) if block.nil?
      __walk__(0, [], block)
      self
    end

    # No arguments is one empty tuple, not none: the recursion bottoms out on
    # the row it has built, and with no sources that row is `[]`.
    def __walk__(at, row, block)
      if at >= @sources.size
        block.call(row.dup)
        return
      end
      @sources[at].each do |value|
        row.push(value)
        __walk__(at + 1, row, block)
        row.pop
      end
    end

    def size
      raise ArgumentError, "uninitialized product" if @sources.nil?
      total = 1
      @sources.each do |source|
        return nil unless source.respond_to?(:size)
        part = source.size
        return nil unless part.is_a?(Integer)
        total = total * part
      end
      total
    end

    def rewind
      __needs_fibers__("rewind")
    end

    def inspect
      return "#<Enumerator::Product: uninitialized>" if @sources.nil?
      "#<Enumerator::Product: #{@sources.inspect}>"
    end

    def to_s
      inspect
    end
  end

  def self.product(*sources, &block)
    made = Product.new(*sources)
    return made if block.nil?
    made.each(&block)
    nil
  end

  # `produce` has no end unless the block raises `StopIteration`, which is what
  # `loop` swallows. Its `size` is `Float::INFINITY` in Ruby and this VM has
  # only flonums, so `size` is left to report the missing constant by name
  # rather than answering `nil`, which would be a wrong answer (#18).
  def self.produce(*initial, &block)
    raise ArgumentError, "no block given" if block.nil?
    if initial.size > 1
      raise ArgumentError, "wrong number of arguments (given #{initial.size}, expected 0..1)"
    end
    new do |y|
      value = initial.empty? ? block.call(nil) : initial[0]
      loop do
        y.yield(value)
        value = block.call(value)
      end
    end
  end

  def +(other)
    Chain.new(self, other)
  end

  # The object a generator block writes into. `y << v` and `y.yield v` both
  # forward one element to whatever block is currently driving `each`.
  class Yielder
    def initialize(&block)
      @block = block
    end

    # One element per call: `y << a, b` is an error, not a two-element yield.
    def <<(*values)
      if values.size != 1
        raise ArgumentError, "wrong number of arguments (given #{values.size}, expected 1)"
      end
      @block.call(values[0])
      self
    end

    # `yield` forwards every value and answers nil — unlike `<<`, which chains.
    def yield(*values)
      @block.call(*values)
      nil
    end

    def call(*values)
      @block.call(*values)
      nil
    end

    def to_proc
      @block
    end
  end
end
