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
