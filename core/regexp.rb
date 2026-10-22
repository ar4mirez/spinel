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
    # `/n` is not part of what a pattern is: `/a/n == /a/`, measured. Its
    # encoding is: `/a/e == /a/s` is false, though both answer 16.
    source == other.source && (options & ~NOENCODING) == (other.options & ~NOENCODING) &&
      encoding == other.encoding
  end

  def eql?(other)
    self == other
  end

  def hash = [source, options & ~NOENCODING, encoding].hash
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
  ESCAPES = Array.new(128)
  " .*?+^$|()[]{}\\-#".each_char { |c| ESCAPES[c.ord] = "\\" + c }
  ESCAPES[0x0a] = "\\n"
  ESCAPES[0x09] = "\\t"
  ESCAPES[0x0d] = "\\r"
  ESCAPES[0x0c] = "\\f"
  ESCAPES[0x0b] = "\\v"
  ESCAPES.freeze
  private_constant :ESCAPES

  # Where no byte of a wider character is below 0x80 — BINARY (index 0),
  # UTF-8 (1), US-ASCII (2), or any ASCII-compatible String that is all ASCII
  # — every byte below 0x80 is a character, so the bytes are walked and the
  # runs between metacharacters copied whole: `gsub` with a String pattern
  # escapes it on every call (#299). Shift_JIS and its kin have trail bytes
  # like `\` and `[`, and are walked a character at a time.
  def self.escape(text)
    text = text.to_s if text.is_a?(Symbol)
    text = String.__coerce__(text)
    unless text.__encoding_index__ <= 2 || (text.encoding.ascii_compatible? && text.ascii_only?)
      return __escape_chars__(text)
    end
    out = String.new(encoding: text.encoding)
    size = text.bytesize
    copied = 0
    i = 0
    while i < size
      byte = text.__getbyte__(i)
      if byte < 0x80 && (escaped = ESCAPES[byte])
        out << text.__byteslice__(copied, i - copied) if i > copied
        out << escaped
        copied = i + 1
      end
      i += 1
    end
    out << text.__byteslice__(copied, size - copied) if size > copied
    out.force_encoding(Encoding::US_ASCII) if out.ascii_only?
    out
  end

  def self.__escape_chars__(text)
    out = String.new(encoding: text.encoding)
    text.each_char do |c|
      escaped = c.bytesize == 1 ? ESCAPES[c.getbyte(0)] : nil
      escaped.nil? ? out << c : out << escaped
    end
    out.force_encoding(Encoding::US_ASCII) if out.ascii_only?
    out
  end

  class << self
    alias quote escape
  end
end

# The rest of the class's surface (#33): what a pattern says about itself,
# and the ways of making one that are not a literal.
class Regexp
  FIXEDENCODING = 16
  NOENCODING = 32

  def casefold?
    options & IGNORECASE != 0
  end

  # The names its groups declare, each once, in order of first appearance.
  def names
    out = []
    table = __name_table__
    index = 0
    while index < table.size
      out.push(table[index]) unless out.include?(table[index])
      index = index + 2
    end
    out
  end

  # Each name and the numbers of the groups it labels.
  def named_captures
    out = {}
    table = __name_table__
    index = 0
    while index < table.size
      (out[table[index]] ||= []).push(table[index + 1])
      index = index + 2
    end
    out
  end

  # The source String is in the pattern's encoding, as CRuby keeps it: US-ASCII
  # for an ASCII pattern free to match in any ASCII-compatible encoding, and
  # the one a modifier, `/n` past ASCII, or the source itself fixed it to.
  def encoding
    source.encoding
  end

  def fixed_encoding?
    options & FIXEDENCODING != 0
  end

  # `~re` matches against `$_`, the last line read.
  def ~
    line = $_
    return nil unless line.is_a?(String)
    self =~ line
  end

  class << self
    # The pattern may be anything that converts with `to_str`, which is a
    # send: it is converted here, and the primitive takes a String or a
    # Regexp.
    alias __new__ new
    def new(*args, timeout: nil)
      unless args.empty? || args[0].is_a?(String) || args[0].is_a?(Regexp)
        pattern = args[0]
        unless pattern.respond_to?(:to_str)
          raise TypeError, "no implicit conversion of " + (pattern.nil? ? "nil" : pattern.class.to_s) + " into String"
        end
        text = pattern.to_str
        unless text.is_a?(String)
          raise TypeError, "can't convert " + pattern.class.to_s + " to String (" +
                           pattern.class.to_s + "#to_str gives " + text.class.to_s + ")"
        end
        args = [text] + args[1, args.size - 1]
      end
      __new__(*args)
    end

    def compile(*args)
      new(*args)
    end

    # One pattern that matches what any of these does. A String is matched
    # literally; a Regexp keeps its own options, inside a group.
    def union(*patterns)
      patterns = patterns[0] if patterns.size == 1 && patterns[0].is_a?(Array)
      return /(?!)/ if patterns.empty?
      if patterns.size == 1
        only = patterns[0]
        return only if only.is_a?(Regexp)
      end
      parts = patterns.map do |pattern|
        if pattern.is_a?(Regexp)
          pattern.to_s
        else
          unless pattern.is_a?(String)
            unless pattern.respond_to?(:to_str)
              raise TypeError, "no implicit conversion of " + pattern.class.to_s + " into String"
            end
            pattern = pattern.to_str
          end
          escape(pattern)
        end
      end
      new(parts.join("|"))
    end

    def try_convert(object)
      return object if object.is_a?(Regexp)
      return nil unless object.respond_to?(:to_regexp)
      converted = object.to_regexp
      return converted if converted.nil? || converted.is_a?(Regexp)
      raise TypeError, "can't convert " + object.class.to_s + " to Regexp (" +
                       object.class.to_s + "#to_regexp gives " + converted.class.to_s + ")"
    end

    # Whether matching is guaranteed linear in the subject. A backreference
    # is what takes that away.
    def linear_time?(pattern, *options)
      if pattern.is_a?(Regexp)
        __send__(:__warning__, "flags ignored") unless options.empty?
      else
        pattern = new(pattern, *options)
      end
      !pattern.source.match?(/\\[1-9]|\\k<|\\g</)
    end

    def timeout
      @__timeout__
    end

    # ponytail: kept and answered, and not enforced. This engine has no
    # deadline to check a match against.
    def timeout=(seconds)
      unless seconds.nil?
        unless seconds.is_a?(Numeric)
          raise TypeError, "can't convert " + seconds.class.to_s + " into Float"
        end
        raise ArgumentError, "invalid timeout: " + seconds.to_s if seconds <= 0
        seconds = seconds.to_f
      end
      @__timeout__ = seconds
    end

    # `$~`, or one group of it.
    def last_match(*group)
      if group.size > 1
        raise ArgumentError, "wrong number of arguments (given " + group.size.to_s + ", expected 0..1)"
      end
      found = $~
      return found if group.empty? || found.nil?
      key = group[0]
      return found[key] if key.is_a?(Symbol) || key.is_a?(String)
      found[Integer.__index__(key)]
    end
  end
end
