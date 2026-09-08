# Regexp.
#
# `=~`, `match`, `match?`, `===`, `source`, `options`, `to_s` and `inspect` are
# primitives: they run the engine in `spinel-regex` and read the compiled
# pattern's table entry.
#
# `Regexp.new` is a primitive too, and a singleton method rather than an
# `initialize`: `Class#allocate` refuses on `Regexp` because a pattern cannot
# exist uninitialised, so there is nothing for the usual allocate-then-initialize
# to allocate. The object it builds is *not* frozen, which is the one way it
# differs from a literal.
class Regexp
  # The flag bits, as Ruby numbers them. `Regexp.new("a", IGNORECASE).options`
  # is 1 and `/a/ix.options` is 3, so these are a public part of the interface
  # rather than an internal encoding.
  IGNORECASE = 1
  EXTENDED = 2
  MULTILINE = 4

  # Reachable only through `send`: `Regexp.new` builds its object without going
  # through `initialize`, so anything that gets here already holds a compiled
  # pattern. A literal is frozen and Ruby raises `FrozenError`; an unfrozen one
  # from `Regexp.new` raises `TypeError`. Measured on ruby 4.0.6 — 4.1 makes
  # both `FrozenError`, and ruby/spec guards the two apart.
  def initialize(*args)
    raise FrozenError, "can't modify frozen Regexp: " + inspect if frozen?
    raise TypeError, "already initialized regexp"
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
