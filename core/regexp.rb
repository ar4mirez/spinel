# Regexp.
#
# `=~`, `match`, `match?`, `===`, `source`, `options`, `to_s` and `inspect` are
# primitives: they run the engine in `spinel-regex` and read the compiled
# pattern's table entry.
#
# `Regexp.new` is not here. Compiling a pattern from a value only known at run
# time needs the per-heap pattern table to be writable from a name the compiler
# never saw, and `allocate` refuses on `Regexp` until it is.
class Regexp
  # Reachable only through `send`: a Regexp literal is frozen from birth, and
  # `Regexp.new` refuses before it gets here. Re-initialising a frozen one is
  # what Ruby raises about, so that is the answer this can give honestly.
  def initialize(*args)
    raise FrozenError, "can't modify frozen Regexp: " + inspect if frozen?
    raise ArgumentError, "Regexp.new is not implemented yet"
  end

  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(Regexp)
    source == other.source && options == other.options
  end

  def eql?(other)
    self == other
  end
end
