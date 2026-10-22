# `Random`, and `Kernel#rand` and `#srand` over a default one (#249).
#
# The generator is MT19937 in `spinel-vm/src/mt.rs`; its state is a BINARY
# String each `Random` holds, so there is one default generator per heap and
# none per process. Everything CRuby's `random.c` decides — which initialiser
# a seed gets, how 32-bit outputs become an Integer below a limit or a Float
# in a range, what each argument means — is here, and was checked against
# ruby 4.0.6's sequences: `Random.new(42).rand(0.0...100.0)` is
# 37.454011884736246 in both.

Random = Class.new(Class.new)
Random.const_set(:Base, Random.superclass)

class Random
  # What `random_number` and `rand` mean for any generator: CRuby's
  # `Random::Formatter`, whose `random_number` answers a Float where `rand`
  # would refuse.
  module Formatter
    def random_number(*args)
      if args.size > 1
        raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)"
      end
      value = __rand__(args)
      return __rand__([]) if value.nil?
      raise ArgumentError, "invalid argument - #{args[0]}" if value == false
      value
    end
    alias rand random_number
  end

  Base.include(Formatter)

  def self.urandom(size)
    size = Integer.__index__(size)
    raise ArgumentError, "negative string size (or size too big)" if size < 0
    return "".b if size == 0
    File.open("/dev/urandom", "rb") { |f| f.read(size) }
  end

  # Four 32-bit words from the operating system, as CRuby's `make_seed_value`.
  def self.new_seed
    urandom(16).unpack("V4").each_with_index.sum { |word, i| word << (32 * i) }
  end

  def self.__default__
    @__default__ ||= new
  end

  def self.seed = __default__.seed
  def self.bytes(size) = __default__.bytes(size)
  def self.rand(*args) = __default__.rand(*args)
  def self.random_number(*args) = __default__.random_number(*args)

  # A new default generator from `seed`; answers the old one's seed.
  def self.srand(seed = new_seed)
    old = __default__.seed
    @__default__ = new(seed)
    old
  end

  # The state for a seed: its absolute value as 32-bit words, least
  # significant first. One word is `init_genrand`'s; more are
  # `init_by_array`'s, with a top word of 1 dropped first — CRuby's guard
  # for a leading zero, measured with 2**32 and 2**32 + 5.
  def self.__state_for__(seed)
    n = seed.abs
    words = []
    while n > 0
      words << (n & 0xffffffff)
      n >>= 32
    end
    return __mt_new__(words.empty? ? [0] : words, true) if words.size <= 1
    words.pop if words.last == 1
    __mt_new__(words, false)
  end

  def initialize(seed = Random.new_seed)
    seed = Integer.__index__(seed) unless Integer === seed
    @seed = seed
    @state = Random.__state_for__(seed)
  end

  def initialize_copy(other)
    @seed = other.seed
    @state = other.__state__.dup
  end

  attr_reader :seed

  def __state__ = @state

  # CRuby's view of the generator: its 624 words as one Integer, least
  # significant first, and how many outputs are left before it regenerates.
  def state
    @state.unpack("V624").each_with_index.sum { |word, i| word << (32 * i) }
  end

  def left = 625 - @state.unpack1("V", offset: 2496)

  def marshal_dump = [state, left, seed]

  def marshal_load(data)
    raise ArgumentError, "wrong dump data" unless Array === data && data.size == 3
    words, rest, seed = data
    rest = Integer.__index__(rest)
    raise ArgumentError, "wrong value" if rest > 624 || rest < 1
    out = Array.new(624) { |i| (words >> (32 * i)) & 0xffffffff }
    @state = (out << (625 - rest)).pack("V*")
    @seed = seed
    self
  end

  private :initialize_copy, :state, :left, :marshal_dump, :marshal_load

  def ==(other)
    other.instance_of?(self.class) && seed == other.seed && @state == other.__state__
  end

  def bytes(size)
    size = Integer.__index__(size)
    raise ArgumentError, "negative string size (or size too big)" if size < 0
    return "".b if size == 0
    __mt_next__(@state, (size + 3) / 4).pack("V*").byteslice(0, size)
  end

  # `genrand_res53`: [0, 1).
  def __real__
    a, b = __mt_next__(@state, 2)
    ((a >> 5) * 67108864.0 + (b >> 6)) / 9007199254740992.0
  end

  # [0, 1], for an inclusive Float range: the 64 bits of two outputs scaled
  # by 2**53 + 1, as CRuby's `int_pair_to_real_inclusive` does. Measured.
  def __real_inclusive__
    a, b = __mt_next__(@state, 2)
    (((a << 32) | b) * 9007199254740993 >> 64) / 9007199254740992.0
  end

  # A uniform Integer in 0..limit: CRuby's `limited_rand` for a limit that
  # fits 64 bits, and `limited_big_rand` past that.
  def __limited__(limit)
    return 0 if limit == 0
    if limit <= 0xffffffff
      # CRuby's `make_mask`: every bit below the top one set, in five steps
      # where `bit_length` takes one per bit.
      mask = limit | (limit >> 1)
      mask = mask | (mask >> 2)
      mask = mask | (mask >> 4)
      mask = mask | (mask >> 8)
      mask = mask | (mask >> 16)
      while true
        value = __mt_next__(@state, 1)[0] & mask
        return value if value <= limit
      end
    end
    mask = (1 << limit.bit_length) - 1
    if limit <= 0xffffffffffffffff
      loop do
        value = 0
        fits = true
        1.downto(0) do |i|
          next if (mask >> (i * 32)) & 0xffffffff == 0
          value |= __mt_next__(@state, 1)[0] << (i * 32)
          value &= mask
          if limit < value
            fits = false
            break
          end
        end
        return value if fits
      end
    else
      limbs = []
      n = limit
      while n > 0
        limbs << (n & 0xffffffff)
        n >>= 32
      end
      loop do
        word_mask = 0
        boundary = true
        out = Array.new(limbs.size, 0)
        again = false
        (limbs.size - 1).downto(0) do |i|
          top = limbs[i]
          word_mask = word_mask != 0 ? 0xffffffff : (1 << top.bit_length) - 1
          next if word_mask == 0
          r = __mt_next__(@state, 1)[0] & word_mask
          if boundary
            if top < r
              again = true
              break
            end
            boundary = false if r < top
          end
          out[i] = r
        end
        next if again
        return out.each_with_index.sum { |word, i| word << (32 * i) }
      end
    end
  end

  # CRuby's `rand_int`: below `max`, nil for zero, and for a negative one nil
  # when `restrictive` (Random#rand) or its absolute value (Kernel#rand).
  def __rand_int__(max, restrictive)
    return nil if max == 0
    if max < 0
      return nil if restrictive
      max = -max
    end
    __limited__(max - 1)
  end

  # CRuby's `rand_range`: false when `range` is no range, nil when it is
  # empty, and EDOM for an open end.
  def __rand_range__(range)
    return false unless Range === range
    first = range.begin
    last = range.end
    raise Errno::EDOM if first.nil? || last.nil?
    excl = range.exclude_end?
    span = last - first
    value = nil
    if !(Float === span) && (Integer === span || span.respond_to?(:to_int))
      span = Integer === span ? span : span.to_int
      top = excl ? span - 1 : span
      value = __limited__(top) if top >= 0
    elsif span.respond_to?(:to_f) && Numeric === span
      max = span.to_f
      scale = 1
      mid = 0.5
      if max.infinite?
        min = first.to_f / 2.0
        max = last.to_f / 2.0
        scale = 2
        mid = max + min
        max -= min
      elsif max.nan?
        raise Errno::EDOM
      end
      if max > 0.0
        r = excl ? __real__ : __real_inclusive__
        return (((r - 0.5) * max) * scale) + mid if scale > 1
        value = r * max
      elsif max == 0.0 && !excl
        value = 0.0
      end
    end
    return nil if value.nil?
    return value + first.to_f if Float === value && first.respond_to?(:to_f) && Numeric === first
    first + value
  end

  # CRuby's `rand_random`: what `Random#rand` answers before it checks.
  def __rand__(args)
    if args.size > 1
      raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)"
    end
    return __real__ if args.empty?
    max = args[0]
    return nil if max.nil?
    if !(Float === max) && (Integer === max || max.respond_to?(:to_int))
      return __rand_int__(Integer === max ? max : max.to_int, true)
    end
    if Numeric === max && max.respond_to?(:to_f)
      limit = max.to_f
      raise Errno::EDOM unless limit.finite?
      return nil if limit < 0.0
      r = __real__
      return limit > 0.0 ? r * limit : r
    end
    __rand_range__(max)
  end

  def rand(*args)
    value = __rand__(args)
    raise ArgumentError, "invalid argument - #{args[0]}" if value.nil?
    # Neither a number nor a range: CRuby converts it, and says why not.
    Integer.__index__(args[0]) if value == false
    value
  end
end

module Kernel
  # `rand(max)`: a range as `Random#rand` takes it, otherwise the Integer
  # `max` converts to — its absolute value, measured — and a Float in [0, 1)
  # when that is zero or there is none.
  def rand(max = nil)
    generator = Random.__default__
    unless max.nil?
      value = generator.__rand_range__(max)
      return value unless value == false
      max = Integer.__index__(Float === max ? max.to_i : max)
      unless max == 0
        value = generator.__rand_int__(max, false)
        return value unless value.nil?
      end
    end
    generator.__real__
  end

  def srand(seed = Random.new_seed)
    Random.srand(seed)
  end

  module_function :rand, :srand
end
