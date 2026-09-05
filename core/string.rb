# String.
#
# `length`, `size`, `bytesize`, `[]`, `+`, `*`, `=~`, `match` and `match?` are
# primitives: they read or allocate a byte payload. Everything below is Ruby.
#
# A String is bytes here, and a character is a UTF-8 codepoint. `force_encoding`,
# `encode` and `b` belong to the Encoding slice, not to this one, and are absent
# rather than approximated.
class String
  def to_s
    self
  end

  def __expect_string__(other)
    return if other.is_a?(String)
    raise TypeError, "no implicit conversion of " + other.class.name + " into String"
  end

  def to_str
    self
  end

  def empty?
    length == 0
  end

  def first_char
    self[0]
  end

  def start_with?(*prefixes)
    i = 0
    while i < prefixes.size
      prefix = prefixes[i]
      return true if self[0, prefix.length] == prefix
      i = i + 1
    end
    false
  end

  def end_with?(*suffixes)
    i = 0
    while i < suffixes.size
      suffix = suffixes[i]
      at = length - suffix.length
      return true if at >= 0 && self[at, suffix.length] == suffix
      i = i + 1
    end
    false
  end

  def include?(other)
    __expect_string__(other)
    return true if other.empty?
    i = 0
    last = length - other.length
    while i <= last
      return true if self[i, other.length] == other
      i = i + 1
    end
    false
  end

  def index(other)
    __expect_string__(other)
    return 0 if other.empty?
    i = 0
    last = length - other.length
    while i <= last
      return i if self[i, other.length] == other
      i = i + 1
    end
    nil
  end

  def each_char
    i = 0
    while i < length
      yield self[i]
      i = i + 1
    end
    self
  end

  def chars
    out = []
    each_char { |c| out.push(c) }
    out
  end

  def reverse
    out = ""
    i = length - 1
    while i >= 0
      out = out + self[i]
      i = i - 1
    end
    out
  end

  def <(other)
    (self <=> other) < 0
  end

  def >(other)
    (self <=> other) > 0
  end

  def <=(other)
    (self <=> other) <= 0
  end

  def >=(other)
    (self <=> other) >= 0
  end

  def inspect
    out = "\""
    i = 0
    while i < length
      c = self[i]
      if c == "\""
        out = out + "\\\""
      elsif c == "\\"
        out = out + "\\\\"
      elsif c == "\n"
        out = out + "\\n"
      elsif c == "\t"
        out = out + "\\t"
      else
        out = out + c
      end
      i = i + 1
    end
    out + "\""
  end
end
