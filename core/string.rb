# String.
#
# `length`, `size`, `bytesize`, `[]`, `+`, `*`, `=~`, `match` and `match?` are
# primitives: they read or allocate a byte payload. Everything below is Ruby.
#
# A String is bytes in an encoding (#19): `core/encoding.rb` names them, and
# the primitives below read and splice the bytes. Characters are walked by
# `__char_offsets__`, which refuses an encoding the VM cannot walk.
class String
  # A subclass's instance answers a plain String copy. Measured.
  def to_s
    instance_of?(String) ? self : String.new(self)
  end

  # `+str` is the receiver unless it is frozen, and then an unfrozen copy.
  def +@
    frozen? ? dup : self
  end

  def __expect_string__(other)
    return if other.is_a?(String)
    raise TypeError, "no implicit conversion of " + other.class.name + " into String"
  end

  def eql?(other)
    other.is_a?(String) && self == other
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
end

# Encodings, bytes and in-place change (#19). Every mutator goes through
# `__modify__`, which is where a frozen String refuses, and through
# `__splice__`, which replaces a byte range; the encoding rules are
# `Encoding.compatible?`'s, raised with CRuby's message.
class String
  def initialize(*given, encoding: nil, capacity: nil)
    if given.size > 1
      raise ArgumentError, "wrong number of arguments (given #{given.size}, expected 0..1)"
    end
    replace(given[0]) unless given.empty?
    force_encoding(encoding) unless encoding.nil?
    self
  end

  def encoding
    Encoding::LIST[__encoding_index__]
  end

  def force_encoding(encoding)
    __modify__
    __force_encoding__(Encoding.find(encoding).__index__)
  end

  # A copy relabelled BINARY. Always a String, never a subclass.
  def b
    copy = String.new(self)
    copy.__force_encoding__(0)
  end

  def __modify__
    Kernel.__check_frozen__(self)
  end

  # The encoding `self` and `other` combine in, or CRuby's
  # `Encoding::CompatibilityError`.
  def __combined_encoding__(other)
    index = __compatible__(other)
    return Encoding::LIST[index] unless index.nil?
    raise Encoding::CompatibilityError,
          "incompatible character encodings: #{encoding.__shown__} and #{other.encoding.__shown__}"
  end

  # --- bytes ------------------------------------------------------------

  def bytes(&block)
    return __bytes__ if block.nil?
    each_byte(&block)
  end

  def each_byte
    return to_enum(:each_byte) { bytesize } unless block_given?
    i = 0
    while i < bytesize
      yield __getbyte__(i)
      i += 1
    end
    self
  end

  def getbyte(index)
    __getbyte__(Integer.__index__(index))
  end

  def setbyte(index, byte)
    __modify__
    index = Integer.__index__(index)
    byte = Integer.__index__(byte)
    at = index < 0 ? index + bytesize : index
    raise IndexError, "index #{index} out of string" if at < 0 || at >= bytesize
    __setbyte__(at, byte)
  end

  def byteslice(*args)
    start, len = __bytes_range__(args, "byteslice")
    return nil if start.nil?
    __byteslice__(start, len)
  end

  # `[start, len]` in bytes, clamped, or nil when the start is out of range —
  # the two-argument and Range forms `byteslice` shares with `[]`.
  def __bytes_range__(args, name)
    size = bytesize
    if args.size == 2
      start = Integer.__index__(args[0])
      len = Integer.__index__(args[1])
      start += size if start < 0
      return nil if start < 0 || start > size || len < 0
      return [start, [len, size - start].min]
    end
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)" unless args.size == 1
    arg = args[0]
    if arg.is_a?(Range)
      start, len = __range_bounds__(arg, size)
      return nil if start.nil?
      return [start, len]
    end
    index = Integer.__index__(arg)
    index += size if index < 0
    return nil if index < 0 || index >= size
    [index, 1]
  end

  # A Range's `[start, length]` against `size`, or nil when it starts out of
  # range — `Array#[]`'s rule, which `String#[]` shares.
  def __range_bounds__(range, size)
    first = range.begin.nil? ? 0 : Integer.__index__(range.begin)
    last = range.end.nil? ? size : Integer.__index__(range.end)
    first += size if first < 0
    last += size if last < 0
    return nil if first < 0 || first > size
    last += 1 unless range.end.nil? || range.exclude_end?
    len = last - first
    len = 0 if len < 0
    [first, [len, size - first].min]
  end

  # --- characters ---------------------------------------------------------

  def [](*args)
    start, len, single = __char_range__(args)
    return start if start.nil? || !start.is_a?(Integer)
    __index__(start, len)
  end

  def slice(*args)
    self[*args]
  end

  # `[start, length]` in characters for `[]` and its family; or `[nil]` when
  # nothing matches; or `[string]` when the argument selects by content and
  # the answer is that content.
  def __char_range__(args)
    size = length
    if args.size == 2
      if args[0].is_a?(Regexp)
        match = args[0].match(self)
        return [nil] if match.nil?
        part = match[args[1]]
        return [nil] if part.nil?
        return [match.begin(args[1]), part.length]
      end
      start = Integer.__index__(args[0])
      len = Integer.__index__(args[1])
      start += size if start < 0
      return [nil] if start < 0 || start > size || len < 0
      return [start, [len, size - start].min]
    end
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 1..2)" unless args.size == 1
    arg = args[0]
    case arg
    when Range
      bounds = __range_bounds__(arg, size)
      return [nil] if bounds.nil?
      bounds
    when String
      at = index(arg)
      at.nil? ? [nil] : [at, arg.length]
    when Regexp
      match = arg.match(self)
      match.nil? ? [nil] : [match.begin(0), match[0].length]
    else
      at = Integer.__index__(arg)
      at += size if at < 0
      return [nil] if at < 0 || at >= size
      [at, 1]
    end
  end

  def []=(*args)
    value = args.pop
    __modify__
    value = String.__coerce__(value)
    if args.size == 1 && args[0].is_a?(String) && index(args[0]).nil?
      raise IndexError, "string not matched"
    end
    if args.size == 1 && args[0].is_a?(Regexp) && args[0].match(self).nil?
      raise IndexError, "regexp not matched"
    end
    if args.size == 1 && !args[0].is_a?(Range) && !args[0].is_a?(String) && !args[0].is_a?(Regexp)
      at = Integer.__index__(args[0])
      if at >= length || at < -length
        raise IndexError, "index #{at} out of string"
      end
    end
    if args.size == 1 && args[0].is_a?(Range)
      first = args[0].begin.nil? ? 0 : Integer.__index__(args[0].begin)
      if first < -length || first > length
        raise RangeError, "#{args[0].inspect} out of range"
      end
    end
    start, len = __char_range__(args)
    if start.nil?
      raise IndexError, "index #{args[0]} out of string"
    end
    __replace_chars__(start, len, value)
    value
  end

  # Replace characters `start...start + len` with `value`, negotiating the
  # encoding first.
  def __replace_chars__(start, len, value)
    combined = __combined_encoding__(value)
    offsets = __char_offsets__
    byte_start = offsets[start]
    byte_end = offsets[[start + len, offsets.size - 1].min]
    __splice__(byte_start, byte_end - byte_start, value)
    __force_encoding__(combined.__index__)
    self
  end

  def slice!(*args)
    __modify__
    start, len = __char_range__(args)
    return nil if start.nil?
    removed = __index__(start, len)
    offsets = __char_offsets__
    byte_start = offsets[start]
    byte_end = offsets[[start + len, offsets.size - 1].min]
    __splice__(byte_start, byte_end - byte_start, "")
    removed
  end

  def insert(index, other)
    __modify__
    other = String.__coerce__(other)
    index = Integer.__index__(index)
    size = length
    at = index < 0 ? index + size + 1 : index
    raise IndexError, "index #{index} out of string" if at < 0 || at > size
    __replace_chars__(at, 0, other)
  end

  def replace(other)
    __modify__
    other = String.__coerce__(other)
    return self if equal?(other)
    __splice__(0, bytesize, other)
    __force_encoding__(other.__encoding_index__)
  end

  def clear
    __modify__
    __splice__(0, bytesize, "")
  end

  def <<(other)
    __modify__
    return __append_codepoint__(other) if other.is_a?(Integer)
    other = String.__coerce__(other)
    combined = __combined_encoding__(other)
    __splice__(bytesize, 0, other)
    __force_encoding__(combined.__index__)
  end

  def concat(*others)
    __modify__
    # Each argument as it was before any was appended: `s.concat(s, s)`
    # triples `s`, it does not quadruple it.
    others = others.map { |other| other.is_a?(Integer) ? other : String.__coerce__(other).dup }
    others.each { |other| self << other }
    self
  end

  def prepend(*others)
    __modify__
    joined = others.map { |other| String.__coerce__(other) }.inject(+"") { |acc, part| acc << part }
    __replace_chars__(0, 0, joined)
  end

  # `s << 233`: a codepoint in a Unicode string, a byte in a binary one, and
  # a binary string from a US-ASCII one given a byte past 127. Measured.
  def __append_codepoint__(code)
    index = __encoding_index__
    if index == 1
      raise RangeError, "#{code} out of char range" if code < 0 || code > 0x10ffff
      if code >= 0xd800 && code <= 0xdfff
        raise RangeError, "invalid codepoint 0x#{Integer.__hex__(code, 1)} in UTF-8"
      end
      __splice__(bytesize, 0, Integer.__utf8__(code))
    elsif index == 0 || index == 2
      raise RangeError, "#{code} out of char range" if code < 0 || code > 255
      __force_encoding__(0) if index == 2 && code > 127
      __splice__(bytesize, 0, Integer.__byte_string__(code))
    else
      raise NotImplementedError, "`<<` of a codepoint in #{encoding.name} needs its character table (#19)"
    end
    self
  end

  def each_char
    return to_enum(:each_char) { length } unless block_given?
    offsets = __char_offsets__
    i = 0
    while i < offsets.size - 1
      yield __byteslice__(offsets[i], offsets[i + 1] - offsets[i])
      i += 1
    end
    self
  end

  def chars(&block)
    return each_char(&block) unless block.nil?
    out = []
    each_char { |c| out.push(c) }
    out
  end

  def reverse
    offsets = __char_offsets__
    out = String.new(encoding: encoding)
    i = offsets.size - 1
    while i > 0
      out.__splice__(out.bytesize, 0, __byteslice__(offsets[i - 1], offsets[i] - offsets[i - 1]))
      i -= 1
    end
    out
  end

  def ord
    raise ArgumentError, "empty string" if empty?
    offsets = __char_offsets__
    char = __byteslice__(0, offsets[1])
    raise ArgumentError, "invalid byte sequence in #{encoding.name}" unless char.valid_encoding?
    return Integer.__utf8_decode__(char.__bytes__) if __encoding_index__ == 1
    char.getbyte(0)
  end

  # --- inspect -------------------------------------------------------------

  # CRuby's `rb_str_inspect`: the named escapes, `\#` before `{`, `$` and `@`,
  # control characters as `\uXXXX` in a Unicode string, and any byte that is
  # not a printable character of the encoding as `\xHH`.
  def inspect
    unicode = __encoding_index__ == 1
    out = +"\""
    offsets = __char_offsets__
    i = 0
    while i < offsets.size - 1
      char = __byteslice__(offsets[i], offsets[i + 1] - offsets[i])
      out << __inspect_char__(char, unicode, offsets, i)
      i += 1
    end
    out << "\""
    out.__force_encoding__(1)
  end

  # Built on first use: this file loads before `Hash`'s Ruby half.
  def self.__escapes__
    return @__escapes__ unless @__escapes__.nil?
    @__escapes__ = { "\"" => "\\\"", "\\" => "\\\\", "\n" => "\\n", "\r" => "\\r", "\t" => "\\t",
                   "\f" => "\\f", "\v" => "\\v", "\b" => "\\b", "\a" => "\\a", "\e" => "\\e" }.freeze
  end

  def __inspect_char__(char, unicode, offsets, i)
    if char.bytesize == 1
      byte = char.getbyte(0)
      named = String.__escapes__[char.b.__force_encoding__(1)] if byte < 0x80
      return named unless named.nil?
      if byte == 0x23 && i + 1 < offsets.size - 1
        following = getbyte(offsets[i + 1])
        return "\\#" if following == 0x7b || following == 0x24 || following == 0x40
      end
      return Integer.__byte_string__(byte).__force_encoding__(1) if byte >= 0x20 && byte < 0x7f
      if byte < 0x80 && unicode
        return "\\u" + Integer.__hex__(byte, 4)
      end
      return "\\x" + Integer.__hex__(byte, 2)
    end
    return char.__bytes__.map { |b| "\\x" + Integer.__hex__(b, 2) }.join unless unicode && char.valid_encoding?
    code = char.ord
    if code >= 0x80 && code <= 0x9f
      return "\\u" + Integer.__hex__(code, 4)
    end
    char
  end

  # The operand a mutator takes: a String, or anything with `to_str`.
  def self.__coerce__(value)
    return value if value.is_a?(String)
    unless value.respond_to?(:to_str)
      raise TypeError, "no implicit conversion of #{value.nil? ? 'nil' : value.class} into String"
    end
    value.to_str
  end
end

class Encoding
  # How messages name an encoding: BINARY as CRuby's `inspect` does.
  def __shown__
    __index__ == 0 ? "BINARY (ASCII-8BIT)" : name
  end
end

class String
  # ASCII letters upcased, every other byte as it is: how encoding names
  # compare. `upcase` itself is Unicode-aware and is not this.
  def __ascii_upcase__
    out = dup
    i = 0
    while i < out.bytesize
      byte = out.__getbyte__(i)
      out.__setbyte__(i, byte - 32) if byte >= 0x61 && byte <= 0x7a
      i += 1
    end
    out
  end
end
