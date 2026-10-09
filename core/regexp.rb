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
    raise FrozenError.new("can't modify frozen Regexp: " + inspect, receiver: self) if frozen?
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

# `===` is Ruby so that a String-like object's `to_str` is asked (#28): a
# Symbol matches as its name, anything else that converts matches as what it
# converts to, and anything that does not is no match — `===` never raises for
# that. Measured on ruby 4.0.7.
class Regexp
  def ===(other)
    text = __case_text__(other)
    if text.nil?
      $~ = nil
      return false
    end
    __case_eq__(text)
  end

  # What `===` matches `other` as, or nil when it is not String-like.
  def __case_text__(other)
    return other if other.is_a?(String)
    return other.to_s if other.is_a?(Symbol)
    return nil unless other.respond_to?(:to_str)
    text = other.to_str
    return text if text.is_a?(String)
    raise TypeError, "can't convert #{other.class} to String (#{other.class}#to_str gives #{text.class})"
  end
end

class Regexp
  # Every metacharacter backslashed, the whitespace escapes spelled out, and
  # `/` left alone — CRuby's set, measured. US-ASCII when the result is ASCII.
  def self.escape(text)
    text = text.to_s if text.is_a?(Symbol)
    text = String.__coerce__(text)
    out = String.new(encoding: text.encoding)
    text.each_char do |c|
      case c
      when " " then out << "\\ "
      when "\n" then out << "\\n"
      when "\t" then out << "\\t"
      when "\r" then out << "\\r"
      when "\f" then out << "\\f"
      when "\v" then out << "\\v"
      when ".", "*", "?", "+", "^", "$", "|", "(", ")", "[", "]", "{", "}", "\\", "-", "#"
        out << "\\" << c
      else out << c
      end
    end
    out.force_encoding(Encoding::US_ASCII) if out.ascii_only?
    out
  end

  class << self
    alias quote escape
  end
end
