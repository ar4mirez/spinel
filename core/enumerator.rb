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
# External iteration — `next`, `peek`, `rewind` — runs `each` on a Fiber; see
# `next` below.
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
    self
  end

  # `to_enum`'s form. Built with `allocate` rather than `new` because Ruby 4.0
  # removed the public receiver/method constructor: `Enumerator.new(1, :upto, 3)`
  # has to raise, and this has to not.
  #
  # An enumerator over a Lazy is a Lazy, as `rb_enumeratorize_with_size` makes
  # it, so `lazy.each_slice(2)` and `lazy.with_object(o)` stay in the chain.
  def self.__for__(object, method, arguments, size, keywords = nil)
    return Lazy.__over__(object, method, arguments, size, keywords) if object.is_a?(Lazy)
    allocate.__init_for__(object, method, arguments, size, keywords)
  end

  # The keywords of the remembered call are kept apart from its positional
  # arguments, so they are keywords again when the call is made: replayed as
  # a trailing Hash, `each_line(chomp: true)` would be two arguments.
  def __init_for__(object, method, arguments, size, keywords = nil)
    @producer = nil
    @object = object
    @method = method
    @arguments = arguments
    @keywords = keywords.nil? || keywords.empty? ? nil : keywords
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
  # Arguments given here are appended to the ones the enumerator was made
  # with: with a block they go to the method, without one they make a new
  # enumerator that holds them.
  def each(*extra, &block)
    if block.nil?
      return self if extra.empty? || !@producer.nil?
      return Enumerator.__for__(@object, @method, @arguments + extra, nil, @keywords)
    end
    if @producer.nil?
      return @object.send(@method, *@arguments, *extra, &block) if @keywords.nil?
      return @object.send(@method, *@arguments, *extra, **@keywords, &block)
    end
    @producer.call(Yielder.new(&block))
  end

  # A nil offset is no offset; anything else is converted with `to_int`.
  def with_index(offset = 0, &block)
    return Enumerator.__for__(self, :with_index, [offset], size) if block.nil?
    __with_index__(offset, &block)
  end

  def each_with_index(&block)
    return Enumerator.__for__(self, :each_with_index, [], size) if block.nil?
    __with_index__(0, &block)
  end

  def __with_index__(offset)
    i = offset.nil? ? 0 : __to_int__(offset)
    # The block's value is the value the driven method sees, and `each` answers
    # what that method answered: `[1, 2, 3].select.each_with_index { false }` is
    # `[]` because `select` got the false and returned its own empty result.
    each do |*a|
      answer = yield(a.size <= 1 ? a[0] : a, i)
      i = i + 1
      answer
    end
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
    elsif (@arguments.nil? || @arguments.empty?) && @keywords.nil?
      "#<Enumerator: #{@object.inspect}:#{@method}>"
    else
      "#<Enumerator: #{@object.inspect}:#{@method}(#{__argument_list__})>"
    end
  end

  def __argument_list__
    parts = []
    @arguments.each { |argument| parts.push(argument.inspect) }
    @keywords.each { |key, value| parts.push(key.to_s + ": " + value.inspect) } unless @keywords.nil?
    parts.join(", ")
  end

  def to_s
    inspect
  end

  # External iteration (#26), on a Fiber (#16): the fiber runs `each`, and
  # every element `each` yields suspends it until the next `next`. That is what
  # a buffer of `to_a` cannot do — `Enumerator.new { loop { y << rand } }.next`
  # has no end to buffer to.
  #
  # `next_values` is the element as the Array of what was yielded — `[]`, `[1]`,
  # `[1, 2]`; `next` unwraps one value and leaves several as the Array. At the
  # end both raise StopIteration carrying `each`'s own return value, and keep
  # raising it until `rewind`. Every rule here was measured on ruby 4.0.7.
  def next
    values = next_values
    values.size <= 1 ? values[0] : values
  end

  def next_values
    unless @__peeked__.nil?
      values = @__peeked__
      @__peeked__ = nil
      return values
    end
    __raise_stop__ if @__finished__
    @__runner__ ||= __runner__
    # A producer that raises ends that iteration, and the next `next` starts a
    # new one: measured, the same exception twice for `2.times { e.next }`.
    begin
      kind, payload = @__runner__.resume
    rescue Exception
      @__runner__ = nil
      raise
    end
    if kind == :done
      @__finished__ = true
      @__result__ = payload
      __raise_stop__
    end
    payload
  end

  def peek
    values = peek_values
    values.size <= 1 ? values[0] : values
  end

  def peek_values
    @__peeked__ = next_values if @__peeked__.nil?
    @__peeked__.dup
  end

  # Start again from the first element, and tell the source to as well if it
  # can: `rewind` is passed on to an object that has one. Answers self.
  def rewind
    @__runner__ = nil
    @__peeked__ = nil
    @__finished__ = nil
    @__result__ = nil
    @__feed__ = nil
    @__fed__ = nil
    @object.rewind if !@object.nil? && @object.respond_to?(:rewind)
    self
  end

  # What the producer's `yield` answers the next time `next` resumes it —
  # once; a second `feed` before that is a TypeError.
  def feed(value)
    raise TypeError, "feed value already set" if @__fed__
    @__feed__ = value
    @__fed__ = true
    nil
  end

  # The fiber that drives `each`, answering `[:value, args]` for every element
  # and `[:done, result]` at the end.
  def __runner__
    Fiber.new do
      result = each do |*args|
        Fiber.yield([:value, args])
        fed = @__feed__
        @__feed__ = nil
        @__fed__ = nil
        fed
      end
      [:done, result]
    end
  end

  def __raise_stop__
    stop = StopIteration.new("iteration reached an end")
    stop.instance_variable_set(:@result, @__result__)
    raise stop
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
  #
  # That table is what a link's *block* sees. What a chain yields to the block
  # that consumes it is packed, whichever link is last — see `__packs__`.
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

    # The other shape: a call remembered, as `to_enum` makes it, with no link.
    # `each` makes the call with the driving block itself, so the block's value
    # reaches the method and the method's value comes back, as it does for an
    # ordinary Enumerator. `Enumerable#lazy` is this over `each`, which is why
    # `(1..3).lazy.each {}` answers the Range.
    def self.__over__(source, method, arguments, size, keywords = nil, description = :__call__)
      allocate.__init_over__(source, method, arguments, size, keywords, description)
    end

    def __init_over__(source, method, arguments, size, keywords, description)
      __init_link__(source, size, nil) { nil }
      @method = method
      @arguments = arguments
      @keywords = keywords.nil? || keywords.empty? ? nil : keywords
      @description = description == :__call__ ? __call_description__ : description
      self
    end

    def __call_description__
      return @method.to_s if @arguments.empty? && @keywords.nil?
      "#{@method}(#{__argument_list__})"
    end

    def __init_link__(source, size, description, &setup)
      @source = source
      @size = size
      @description = description
      @setup = setup
      @packs = false
      @object = nil
      @method = nil
      @producer = nil
      self
    end

    # A link a method made, as opposed to one `Lazy.new` was given. CRuby
    # packs what such a chain yields, measured on 4.0.7: inside the chain a
    # link sees `yield` as nil and `yield 1, 2` as two values, and the block
    # that finally consumes it sees one value, `nil` or `[1, 2]`.
    def __packs__
      @packs = true
      self
    end

    # Arguments go to the source's `each`, as the generator CRuby builds passes
    # them; a link answers nil, as that generator does.
    def each(*extra, &block)
      raise ArgumentError, "uninitialized enumerator" if @setup.nil?
      return self if block.nil?
      return __drive__(extra, &block) unless @packs
      __drive__(extra) { |*values| block.call(__pack__(values)) }
    end

    # Drives the source through this run's link, without the packing `each`
    # does at the end, which is what a link above this one wants. The `catch`
    # is what lets a link stop iteration from arbitrarily deep inside a nested
    # chain, which a `return` cannot do: it would leave the link, not the
    # enumeration.
    def __drive__(extra, &block)
      unless @method.nil?
        return @source.send(@method, *@arguments, *extra, &block) if @keywords.nil?
        return @source.send(@method, *@arguments, *extra, **@keywords, &block)
      end
      tag = Object.new
      link = @setup.call(tag)
      yielder = Yielder.new(&block)
      packs = @packs
      feed = proc do |*values|
        values = [nil] if packs && values.empty?
        link.call(yielder, *values)
      end
      source = @source
      catch(tag) { source.is_a?(Lazy) ? source.__drive__(extra, &feed) : source.each(*extra, &feed) }
      nil
    end

    def size
      raise ArgumentError, "uninitialized enumerator" if @setup.nil?
      # A Proc size is asked each time, as `Enumerator#size` asks it.
      @size.respond_to?(:call) ? @size.call : @size
    end

    def lazy
      self
    end

    # `eager` is the one link that leaves the chain: an ordinary Enumerator
    # over the same elements, so `map` on it answers an Array again.
    def eager
      Enumerator.allocate.__init_for__(self, :each, [], @size)
    end

    # The grouping methods stay lazy, measured: `lazy.chunk { }` is a Lazy.
    # Enumerable's versions already defer until iterated, so a lazy over what
    # they answer is the same chain.
    def chunk(*given, &block)
      super.lazy
    end

    def chunk_while(*given, &block)
      super.lazy
    end

    def slice_after(*given, &block)
      super.lazy
    end

    def slice_before(*given, &block)
      super.lazy
    end

    def slice_when(*given, &block)
      super.lazy
    end

    # The methods this class makes lazy, under the names CRuby gives their
    # eager originals. `to_enum(:with_index, 10)` names one of them, and means
    # the eager method — driving the lazy `with_index` would only build another
    # link — so `to_enum` swaps the name, as `lazy_use_super_method` does.
    ENUMERABLE_METHODS = %i[
      map collect flat_map collect_concat select find_all filter filter_map
      reject grep grep_v zip take take_while drop drop_while uniq with_index
    ]
    ENUMERABLE_METHODS.each { |name| alias_method(:"_enumerable_#{name}", name) }
    private(*ENUMERABLE_METHODS.map { |name| :"_enumerable_#{name}" })

    # `to_enum` on a lazy stays lazy — that is the whole point of `eager`
    # existing as the way out. A named method is honoured rather than ignored:
    # `(1..6).lazy.to_enum(:each_slice, 2).first(2)` is `[[1, 2], [3, 4]]`, so
    # the lazy drives that method and not `each`. Its size is the block's,
    # called with the arguments, or not known.
    def to_enum(method = :each, *arguments, **keywords, &size_block)
      method = :"_enumerable_#{method}" if ENUMERABLE_METHODS.include?(method)
      size = size_block.nil? ? nil : -> { size_block.call(*arguments) }
      Lazy.__over__(self, method, arguments, size, keywords)
    end

    def enum_for(method = :each, *arguments, **keywords, &size_block)
      to_enum(method, *arguments, **keywords, &size_block)
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

    # `lazy_flat_map_proc`: an Array is spread, so is anything answering both
    # `force` and `each` — a lazy — and then anything with a `to_ary`.
    def __spread__(yielder, value)
      if !value.is_a?(Array) && value.respond_to?(:force) && value.respond_to?(:each)
        value.each { |element| yielder.yield(element) }
        return
      end
      list = value.is_a?(Array) ? value : Array.try_convert(value)
      if list.nil?
        yielder.yield(value)
      else
        list.each { |element| yielder.yield(element) }
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

    # Asked when the size is, not when the link is made: `lazy_take_size`.
    def __take_size__(wanted)
      -> do
        known = size
        known.nil? || (known.is_a?(Integer) && known < wanted) ? known : wanted
      end
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
      -> do
        known = size
        next known if known.nil?
        next known - dropped_count unless known.is_a?(Integer)
        known > dropped_count ? known - dropped_count : 0
      end
    end

    # With a block, the block runs for its side effect and its value is
    # *discarded* — the element passes through unchanged. Measured, not
    # guessed: `(10..13).lazy.with_index(1) { "X" }.first(3)` is `[10, 11, 12]`
    # in Ruby, where mapping the block's value would answer `["X", "X", "X"]`.
    # Without a block the element becomes the `[item, index]` pair instead.
    def with_index(offset = 0, &block)
      start = offset.nil? ? 0 : __to_int__(offset)
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

    # Arrays are indexed; anything else is read with `next`, one external
    # iterator per run, and a finished one answers nil from then on.
    def zip(*others)
      return super if block_given?
      description = "zip(#{others.map(&:inspect).join(", ")})"
      if others.all? { |other| other.is_a?(Array) }
        return __step__(@size, description) do
          at = 0
          ->(y, *values) do
            row = [__pack__(values)]
            others.each { |other| row.push(other[at]) }
            at = at + 1
            y.yield(row)
          end
        end
      end
      others.each do |other|
        next if other.respond_to?(:each)
        raise TypeError, "wrong argument type #{other.class} (must respond to :each)"
      end
      __step__(@size, description) do
        iterators = nil
        ->(y, *values) do
          iterators = others.map { |other| other.to_enum(:each) } if iterators.nil?
          row = [__pack__(values)]
          iterators.each do |iterator|
            begin
              row.push(iterator.next)
            rescue StopIteration
              row.push(nil)
            end
          end
          y.yield(row)
        end
      end
    end

    # --- plumbing --------------------------------------------------------------

    def __step__(size, description, &setup)
      Lazy.__link__(self, size, description) { |_tag| setup.call }.__packs__
    end

    def __bounded__(size, description, &setup)
      Lazy.__link__(self, size, description) { |tag| setup.call(tag) }.__packs__
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
      self
    end

    def each(&block)
      raise ArgumentError, "uninitialized chain" if @sources.nil?
      return to_enum(:each) if block.nil?
      @sources.each_with_index do |source, index|
        @__started__ = index + 1 if (@__started__ || 0) < index + 1
        source.each { |*values| block.call(*values) }
      end
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
        # The first link without a count decides: nil, or Infinity.
        return part unless part.is_a?(Integer)
        total = total + part
      end
      total
    end

    # The sources iterated so far, last first, each that has a `rewind`.
    # Measured: a chain never run rewinds nothing.
    def rewind
      started = @__started__ || 0
      @sources.__take__(0, started).reverse_each do |source|
        source.rewind if source.respond_to?(:rewind)
      end
      @__started__ = 0
      self
    end

    # A chain is iterated with `each`; CRuby undefines external iteration.
    undef_method :next, :peek, :next_values, :peek_values, :feed

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
      self
    end

    # The sources, shared rather than copied, as CRuby's `dup` does.
    def initialize_copy(other)
      return self if equal?(other)
      super
      raise ArgumentError, "uninitialized product" if other.__sources__.nil?
      @sources = other.__sources__
      self
    end

    def __sources__
      @sources
    end

    def each(&block)
      raise ArgumentError, "uninitialized product" if @sources.nil?
      return to_enum(:each) { size } if block.nil?
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
      @sources[at].each_entry do |value|
        row.push(value)
        __walk__(at + 1, row, block)
        row.pop
      end
    end

    def size
      raise ArgumentError, "uninitialized product" if @sources.nil?
      total = 1
      endless = false
      @sources.each do |source|
        return nil unless source.respond_to?(:size)
        part = source.size
        if part.is_a?(Float) && part.infinite?
          endless = true
          next
        end
        return nil unless part.is_a?(Integer)
        total = total * part
      end
      endless ? Float::INFINITY : total
    end

    # Every source that has a `rewind`, in order. Measured.
    def rewind
      @sources.each { |source| source.rewind if source.respond_to?(:rewind) }
      self
    end

    undef_method :next, :peek, :next_values, :peek_values, :feed

    def inspect
      return "#<Enumerator::Product: uninitialized>" if @sources.nil?
      Kernel.__inspect_guard__(self, "#<Enumerator::Product: ...>") do
        "#<Enumerator::Product: #{@sources.inspect}>"
      end
    end

    def to_s
      inspect
    end
  end

  # No keywords, but it takes `**` so that one is reported by name rather than
  # arriving as a trailing Hash to multiply by. Measured wording.
  def self.product(*sources, **unknown, &block)
    Enumerator.__unknown_keywords__(unknown)
    made = Product.new(*sources)
    return made if block.nil?
    made.each(&block)
    nil
  end

  # `produce` has no end unless the block raises `StopIteration`, which is what
  # `loop` swallows, so its `size` is `Float::INFINITY`.
  #
  # Ruby 4.0 takes `size:`, which is the size the enumerator reports.
  def self.produce(*initial, size: :__spinel_no_size__, **unknown, &block)
    Enumerator.__unknown_keywords__(unknown)
    raise ArgumentError, "no block given" if block.nil?
    if initial.size > 1
      raise ArgumentError, "wrong number of arguments (given #{initial.size}, expected 0..1)"
    end
    sized = size.equal?(:__spinel_no_size__) ? [Float::INFINITY] : [size]
    new(*sized) do |y|
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

  def self.__unknown_keywords__(unknown)
    return if unknown.empty?
    names = unknown.keys.map(&:inspect).join(", ")
    raise ArgumentError, (unknown.size == 1 ? "unknown keyword: " : "unknown keywords: ") + names
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

    # `yield` forwards every value and answers what the consuming block did —
    # unlike `<<`, which chains. Measured: `Enumerator.new { |y| p y.yield(1) }
    # .each { :z }` prints `:z`, and it is how `feed` reaches the producer.
    # (There is no `Yielder#call` in CRuby.)
    def yield(*values)
      @block.call(*values)
    end

    def to_proc
      @block
    end
  end
end
