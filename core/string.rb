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

  # Two Strings are compared by the operator itself and never arrive here
  # through `==`; a send does, and so does anything that is not a String.
  # One with `to_str` is asked, `other == self`, and its answer is made a
  # boolean. Measured.
  def ==(other)
    return self == other if other.is_a?(String)
    return false unless other.respond_to?(:to_str)
    other == self ? true : false
  end
  alias === ==

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
    return __wide_inspect__ if [3, 4, 5, 6].include?(__encoding_index__)
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

  # A UTF-16 or UTF-32 string, through its codepoints: ASCII as itself or its
  # named escape, everything else `\uXXXX` or `\u{XXXXX}`, and an invalid
  # character as its bytes. Measured.
  def __wide_inspect__
    out = +"\""
    each_char do |char|
      unless char.valid_encoding?
        char.__bytes__.each { |b| out << "\\x" << Integer.__hex__(b, 2) }
        next
      end
      code = char.encode(Encoding::UTF_8).ord
      if code < 0x80
        ascii = Integer.__byte_string__(code).__force_encoding__(1)
        named = String.__escapes__[ascii]
        if named
          out << named
        elsif code >= 0x20 && code < 0x7f
          out << ascii
        else
          out << "\\u" << Integer.__hex__(code, 4)
        end
      elsif code > 0xffff
        out << "\\u{" << Integer.__hex__(code, 1) << "}"
      else
        out << "\\u" << Integer.__hex__(code, 4)
      end
    end
    out << "\""
    out
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

# The long tail (#19, second half). Ruby over the byte primitives: search is
# `__byte_index__`, case is `__case_map__`, and every position a method
# answers is converted to characters through `__char_offsets__`.
class String
  # --- positions -------------------------------------------------------------

  def __byte_to_char__(byte)
    offsets = __char_offsets__
    lo = 0
    hi = offsets.size - 1
    while lo < hi
      mid = (lo + hi) / 2
      if offsets[mid] < byte
        lo = mid + 1
      else
        hi = mid
      end
    end
    lo
  end

  def __char_to_byte__(char)
    offsets = __char_offsets__
    char >= offsets.size ? bytesize : offsets[char]
  end

  # --- searching -------------------------------------------------------------

  def index(pattern, start = 0)
    start = Integer.__index__(start)
    size = length
    start += size if start < 0
    if start < 0 || start > size
      $~ = nil if pattern.is_a?(Regexp)
      return nil
    end
    if pattern.is_a?(Regexp)
      match = pattern.match(self, start)
      return match.nil? ? nil : match.begin(0)
    end
    pattern = String.__coerce__(pattern)
    __combined_encoding__(pattern)
    at = __byte_index__(pattern, __char_to_byte__(start))
    at.nil? ? nil : __byte_to_char__(at)
  end

  def rindex(pattern, *rest)
    raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 1..2)" if rest.size > 1
    size = length
    if rest.empty?
      start = size
    else
      start = Integer.__index__(rest[0])
      start += size if start < 0
      return nil if start < 0
      start = size if start > size
    end
    if pattern.is_a?(Regexp)
      at = start
      while at >= 0
        match = pattern.match(self, at)
        return at if !match.nil? && match.begin(0) == at
        at -= 1
      end
      return nil
    end
    pattern = String.__coerce__(pattern)
    __combined_encoding__(pattern)
    at = __byte_rindex__(pattern, __char_to_byte__(start))
    at.nil? ? nil : __byte_to_char__(at)
  end

  def byteindex(pattern, start = 0)
    start = Integer.__index__(start)
    start += bytesize if start < 0
    return nil if start < 0 || start > bytesize
    __check_char_boundary__(start)
    if pattern.is_a?(Regexp)
      match = pattern.match(self, __byte_to_char__(start))
      return match.nil? ? nil : match.byteoffset(0)[0]
    end
    pattern = String.__coerce__(pattern)
    __combined_encoding__(pattern)
    __byte_index__(pattern, start)
  end

  def byterindex(pattern, *rest)
    raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 1..2)" if rest.size > 1
    if rest.empty?
      start = bytesize
    else
      start = Integer.__index__(rest[0])
      start += bytesize if start < 0
      return nil if start < 0
      start = bytesize if start > bytesize
    end
    __check_char_boundary__(start)
    pattern = String.__coerce__(pattern)
    __combined_encoding__(pattern)
    __byte_rindex__(pattern, start)
  end

  def __check_char_boundary__(byte)
    return if byte == bytesize || __char_offsets__.include?(byte)
    raise IndexError, "offset #{byte} does not land on character boundary"
  end

  def include?(other)
    !index(String.__coerce__(other)).nil?
  end

  def start_with?(*prefixes)
    prefixes.any? do |prefix|
      if prefix.is_a?(Regexp)
        match = prefix.match(self)
        !match.nil? && match.begin(0) == 0
      else
        prefix = String.__coerce__(prefix)
        bytesize >= prefix.bytesize && __byteslice__(0, prefix.bytesize) == prefix &&
          (prefix.empty? || __char_offsets__.include?(prefix.bytesize))
      end
    end
  end

  def end_with?(*suffixes)
    suffixes.any? do |suffix|
      suffix = String.__coerce__(suffix)
      at = bytesize - suffix.bytesize
      at >= 0 && __byteslice__(at, suffix.bytesize) == suffix &&
        (suffix.empty? || __char_offsets__.include?(at))
    end
  end

  # A copy without the prefix or suffix, always a plain String; the bang forms
  # answer nil when there was none. Measured.
  def delete_prefix(prefix)
    prefix = String.__coerce__(prefix)
    start_with?(prefix) ? __byteslice__(prefix.bytesize, bytesize - prefix.bytesize) : __byteslice__(0, bytesize)
  end

  def delete_suffix(suffix)
    suffix = String.__coerce__(suffix)
    end_with?(suffix) ? __byteslice__(0, bytesize - suffix.bytesize) : __byteslice__(0, bytesize)
  end

  def delete_prefix!(prefix)
    __modify__
    prefix = String.__coerce__(prefix)
    return nil if prefix.empty? || !start_with?(prefix)
    replace(delete_prefix(prefix))
  end

  def delete_suffix!(suffix)
    __modify__
    suffix = String.__coerce__(suffix)
    return nil if suffix.empty? || !end_with?(suffix)
    replace(delete_suffix(suffix))
  end

  def partition(pattern)
    if pattern.is_a?(Regexp)
      match = pattern.match(self)
      return [__byteslice__(0, bytesize), "", ""] if match.nil?
      return [match.pre_match, match[0], match.post_match]
    end
    pattern = String.__coerce__(pattern)
    at = index(pattern)
    return [__byteslice__(0, bytesize), String.new("", encoding: encoding), String.new("", encoding: encoding)] if at.nil?
    # The middle is the pattern itself, in its own encoding. Measured.
    [self[0, at], pattern.dup, self[at + pattern.length..]]
  end

  def rpartition(pattern)
    if pattern.is_a?(Regexp)
      at = rindex(pattern)
      return ["", "", __byteslice__(0, bytesize)] if at.nil?
      match = pattern.match(self, at)
      return [self[0, at], match[0], self[at + match[0].length..]]
    end
    pattern = String.__coerce__(pattern)
    at = rindex(pattern)
    return [String.new("", encoding: encoding), String.new("", encoding: encoding), __byteslice__(0, bytesize)] if at.nil?
    [self[0, at], pattern.dup, self[at + pattern.length..]]
  end

  # --- trimming and padding --------------------------------------------------

  # What `strip` removes from either end: ASCII whitespace and NUL. Measured.
  def __strippable__(byte)
    byte == 0x20 || (byte >= 0x09 && byte <= 0x0d) || byte == 0
  end

  # An invalid character where stripping stops is an error — ArgumentError on
  # the left and, measured, Encoding::CompatibilityError on the right.
  def lstrip
    start = 0
    start += 1 while start < bytesize && __strippable__(__getbyte__(start))
    if start < bytesize && !__byteslice__(start, __char_len_at__(start)).valid_encoding?
      raise ArgumentError, "invalid byte sequence in #{encoding.name}"
    end
    __byteslice__(start, bytesize - start)
  end

  def __char_len_at__(byte)
    offsets = __char_offsets__
    at = offsets.index(byte)
    at.nil? ? 1 : offsets[at + 1] - byte
  end

  def rstrip
    stop = bytesize
    stop -= 1 while stop > 0 && __strippable__(__getbyte__(stop - 1))
    if stop > 0
      offsets = __char_offsets__
      last_start = offsets.select { |offset| offset < stop }.last
      unless __byteslice__(last_start, stop - last_start).valid_encoding?
        raise Encoding::CompatibilityError, "invalid byte sequence in #{encoding.name}"
      end
    end
    stop = bytesize
    stop -= 1 while stop > 0 && __strippable__(__getbyte__(stop - 1))
    __byteslice__(0, stop)
  end

  def strip
    rstrip.lstrip
  end

  def lstrip!
    __bang__(lstrip)
  end

  def rstrip!
    __bang__(rstrip)
  end

  def strip!
    __bang__(strip)
  end

  # A bang method's ending: replace self and answer it, or nil when nothing
  # changed. The frozen check comes first, changed or not. Measured.
  def __bang__(result)
    __modify__
    return nil if result == self && result.__encoding_index__ == __encoding_index__
    replace(result)
  end

  # The default separator is `$/`, which is "\n" until a program changes it.
  def chomp(*args)
    raise ArgumentError, "wrong number of arguments (given #{args.size}, expected 0..1)" if args.size > 1
    separator = args.empty? ? $/ : args[0]
    return dup if separator.nil?
    return __wide_chomp__(separator) unless encoding.ascii_compatible?
    separator = String.__coerce__(separator)
    return dup if empty?
    if separator == "\n"
      if __getbyte__(bytesize - 1) == 0x0a
        cut = bytesize >= 2 && __getbyte__(bytesize - 2) == 0x0d ? 2 : 1
        return __byteslice__(0, bytesize - cut)
      end
      return __byteslice__(0, bytesize - 1) if __getbyte__(bytesize - 1) == 0x0d
      return dup
    end
    if separator.empty?
      stop = bytesize
      while stop > 0 && __getbyte__(stop - 1) == 0x0a
        stop -= 1
        stop -= 1 if stop > 0 && __getbyte__(stop - 1) == 0x0d
      end
      return __byteslice__(0, stop)
    end
    end_with?(separator) ? __byteslice__(0, bytesize - separator.bytesize) : dup
  end

  def chomp!(*args)
    __bang__(chomp(*args))
  end

  # `chomp` in an encoding whose newline is not the byte 0x0A — UTF-16 and
  # UTF-32 — by characters rather than bytes.
  def __wide_chomp__(separator)
    separator = String.__coerce__(separator)
    lf = "\n".encode(encoding)
    cr = "\r".encode(encoding)
    unless separator == "\n" || separator == lf
      __combined_encoding__(separator)
      return end_with?(separator) ? self[0, length - separator.length] : dup
    end
    return self[0, length - 2] if length >= 2 && self[-2] == cr && self[-1] == lf
    return self[0, length - 1] if !empty? && (self[-1] == lf || self[-1] == cr)
    dup
  end

  def chop
    return dup if empty?
    unless encoding.ascii_compatible?
      return self[0, length - 2] if length >= 2 && self[-2] == "\r".encode(encoding) && self[-1] == "\n".encode(encoding)
      return self[0, length - 1]
    end
    if bytesize >= 2 && __getbyte__(bytesize - 1) == 0x0a && __getbyte__(bytesize - 2) == 0x0d
      return __byteslice__(0, bytesize - 2)
    end
    offsets = __char_offsets__
    __byteslice__(0, offsets[offsets.size - 2])
  end

  def chop!
    __bang__(chop)
  end

  def __pad__(width, padding, side)
    width = Integer.__index__(width)
    padding = String.__coerce__(padding)
    raise ArgumentError, "zero width padding" if padding.empty?
    size = length
    return dup if width <= size
    total = width - size
    left = side == :left ? total : (side == :right ? 0 : total / 2)
    right = total - left
    pad_chars = padding.chars
    out = String.new(encoding: __combined_encoding__(padding))
    left.times { |i| out << pad_chars[i % pad_chars.size] }
    out << self
    right.times { |i| out << pad_chars[i % pad_chars.size] }
    out
  end

  def center(width, padding = " ")
    __pad__(width, padding, :center)
  end

  def ljust(width, padding = " ")
    __pad__(width, padding, :right)
  end

  def rjust(width, padding = " ")
    __pad__(width, padding, :left)
  end

  # --- case ------------------------------------------------------------------

  # CRuby's `check_case_options`: at most two, `:ascii` and `:fold` alone,
  # `:turkic` and `:lithuanian` only with each other, and `:fold` only for
  # `downcase`. Measured.
  def __case_options__(options, folding = false)
    raise ArgumentError, "too many options" if options.size > 2
    ascii = false
    turkic = false
    fold = false
    options.each do |option|
      case option
      when :ascii then ascii = true
      when :turkic then turkic = true
      when :lithuanian then nil
      when :fold then fold = true
      else raise ArgumentError, "invalid option: #{option.inspect}"
      end
    end
    if options.size == 2
      pair = options.sort_by { |option| option.to_s }
      raise ArgumentError, "invalid second option" unless pair == [:lithuanian, :turkic]
    end
    raise ArgumentError, "option :fold only allowed for downcasing" if fold && !folding
    [ascii, turkic, fold]
  end

  def upcase(*options)
    ascii, turkic, = __case_options__(options)
    __case_map__(0, ascii, turkic)
  end

  def downcase(*options)
    ascii, turkic, fold = __case_options__(options, true)
    __case_map__(fold ? 4 : 1, ascii, turkic)
  end

  def swapcase(*options)
    ascii, turkic, = __case_options__(options)
    __case_map__(2, ascii, turkic)
  end

  def capitalize(*options)
    ascii, turkic, = __case_options__(options)
    __case_map__(3, ascii, turkic)
  end

  def upcase!(*options)
    __bang__(upcase(*options))
  end

  def downcase!(*options)
    __bang__(downcase(*options))
  end

  def swapcase!(*options)
    __bang__(swapcase(*options))
  end

  def capitalize!(*options)
    __bang__(capitalize(*options))
  end

  # ASCII-only, by bytes: `nil` for something that does not convert, and for
  # two strings whose encodings cannot be compared.
  def casecmp(other)
    other = __casecmp_operand__(other)
    return nil if other.nil?
    return nil if Encoding.compatible?(self, other).nil?
    downcase(:ascii) <=> other.downcase(:ascii)
  end

  # A String, a Symbol's name, or `to_str`'s answer; nil for anything else,
  # which `casecmp` answers as nil rather than raising. Measured.
  def __casecmp_operand__(other)
    return other if other.is_a?(String)
    return other.to_s if other.is_a?(Symbol)
    other.respond_to?(:to_str) ? other.to_str : nil
  end

  def casecmp?(other)
    other = __casecmp_operand__(other)
    return nil if other.nil?
    return nil if Encoding.compatible?(self, other).nil?
    downcase(:fold) == other.downcase(:fold)
  end
end

class String
  # --- splitting -------------------------------------------------------------

  # A port of CRuby's `rb_str_split_m`, so its rules come with it: a limit
  # of 1 answers the string whole; a positive limit caps the fields and keeps
  # the remainder as the last; zero drops trailing empty fields; negative
  # keeps them. Awk mode (a single space, or nil) skips runs of ASCII
  # whitespace — not NUL; an empty string separator splits characters; a
  # Regexp splices in every participating group. Measured.
  def split(pattern = nil, *rest, &block)
    raise ArgumentError, "wrong number of arguments (given #{rest.size + 1}, expected 0..2)" if rest.size > 1
    lim = rest.empty? ? 0 : Integer.__index__(rest[0])
    if lim > 2147483647 || lim < -2147483648
      raise RangeError, "integer #{lim} too big to convert to 'int'"
    end
    if pattern.nil? && !$;.nil?
      __warning__("$; is set to non-nil value", false, :deprecated)
      pattern = $;
    end
    unless pattern.nil? || pattern.is_a?(Regexp)
      pattern = String.__coerce__(pattern)
      unless pattern.valid_encoding?
        raise ArgumentError, "invalid byte sequence in #{pattern.encoding.name}"
      end
    end
    if !valid_encoding? && __encoding_index__ != 0
      raise ArgumentError, "invalid byte sequence in #{encoding.name}"
    end
    pieces =
      if empty?
        []
      elsif lim == 1
        [__byteslice__(0, bytesize)]
      elsif pattern.nil? || (pattern.is_a?(String) && pattern == " ")
        __split_awk__(lim)
      elsif pattern.is_a?(Regexp)
        __split_regexp__(pattern, lim)
      elsif pattern.empty?
        __split_chars__(lim)
      else
        __split_string__(pattern, lim)
      end
    pieces.pop while lim == 0 && !pieces.empty? && pieces.last.empty?
    return pieces if block.nil?
    pieces.each(&block)
    self
  end

  def __split_space__(byte)
    byte == 0x20 || (byte >= 0x09 && byte <= 0x0d)
  end

  # The field after the last separator: always under a positive limit, and
  # otherwise only when it is not empty or the limit is negative.
  def __split_rest__(pieces, beg, lim)
    if lim > 0 || bytesize > beg || lim < 0
      pieces.push(__byteslice__(beg, bytesize - beg))
    end
    pieces
  end

  def __split_awk__(lim)
    pieces = []
    beg = 0
    count = 1
    skip = true
    ptr = 0
    finish = nil
    size = bytesize
    while ptr < size
      if __split_space__(__getbyte__(ptr))
        if skip
          beg = ptr + 1
        else
          pieces.push(__byteslice__(beg, finish - beg))
          skip = true
          beg = ptr + 1
          count += 1
          break if lim > 0 && lim <= count
        end
      else
        skip = false
        finish = ptr + 1
      end
      ptr += 1
    end
    __split_rest__(pieces, beg, lim)
  end

  def __split_chars__(lim)
    pieces = []
    offsets = __char_offsets__
    count = 1
    beg = 0
    i = 0
    while i < offsets.size - 1
      pieces.push(__byteslice__(offsets[i], offsets[i + 1] - offsets[i]))
      beg = offsets[i + 1]
      i += 1
      count += 1
      break if lim > 0 && lim <= count
    end
    __split_rest__(pieces, beg, lim)
  end

  def __split_string__(separator, lim)
    __combined_encoding__(separator)
    pieces = []
    beg = 0
    count = 1
    while (found = __byte_index__(separator, beg))
      pieces.push(__byteslice__(beg, found - beg))
      beg = found + separator.bytesize
      count += 1
      break if lim > 0 && lim <= count
    end
    __split_rest__(pieces, beg, lim)
  end

  # Character positions, as `MatchData` answers them; an empty match at the
  # current start is skipped once and then splits off one character.
  def __split_regexp__(pattern, lim)
    pieces = []
    size = length
    beg = 0
    start = 0
    last_null = false
    count = 1
    while start <= size && !(match = pattern.match(self, start)).nil?
      first = match.begin(0)
      if start == first && match.begin(0) == match.end(0)
        if last_null
          pieces.push(self[beg, 1])
          beg = start
        else
          start += 1
          last_null = true
          next
        end
      else
        pieces.push(self[beg, first - beg])
        beg = start = match.end(0)
      end
      last_null = false
      (1...match.size).each { |group| pieces.push(match[group]) unless match.begin(group).nil? }
      count += 1
      break if lim > 0 && lim <= count
    end
    if lim > 0 || size > beg || lim < 0
      pieces.push(beg >= size ? String.new(encoding: encoding) : self[beg..])
    end
    pieces
  end

  def each_line(separator = $/, chomp: false, &block)
    return to_enum(:each_line, separator, chomp: chomp) if block.nil?
    __lines__(separator, chomp).each(&block)
    self
  end

  def lines(separator = $/, chomp: false, &block)
    return each_line(separator, chomp: chomp, &block) unless block.nil?
    __lines__(separator, chomp)
  end

  # Lines keep their separator unless `chomp:`; an empty separator is
  # paragraph mode, splitting on runs of blank lines.
  def __lines__(separator, chomp)
    return [chomp ? self.chomp : dup] if separator.nil?
    separator = String.__coerce__(separator)
    paragraph = separator.empty?
    separator = "\n\n" if paragraph
    lines = []
    at = 0
    while at < bytesize
      found = __byte_index__(separator, at)
      width = separator.bytesize
      # A paragraph also ends at a newline followed by a CRLF blank line.
      # Measured: `"a\r\n\r\nb".lines("")` is `["a\r\n\r\n", "b"]`.
      if paragraph
        crlf = __byte_index__("\n\r\n", at)
        found, width = crlf, 3 if crlf && (found.nil? || crlf < found)
      end
      if found.nil?
        lines.push(__byteslice__(at, bytesize - at))
        break
      end
      stop = found + width
      line = __byteslice__(at, (chomp ? found : stop) - at)
      line = line.__byteslice__(0, line.bytesize - 1) if chomp && !paragraph && separator == "\n" && line.end_with?("\r")
      lines.push(line)
      # Paragraph mode swallows the rest of a blank run without keeping it.
      while paragraph && stop < bytesize
        if __getbyte__(stop) == 0x0a
          stop += 1
        elsif __getbyte__(stop) == 0x0d && stop + 1 < bytesize && __getbyte__(stop + 1) == 0x0a
          stop += 2
        else
          break
        end
      end
      at = stop
    end
    lines
  end

  # --- regexp substitution ---------------------------------------------------

  def scan(pattern, &block)
    pattern = Regexp.new(Regexp.escape(String.__coerce__(pattern))) unless pattern.is_a?(Regexp)
    found = []
    at = 0
    last_match = nil
    while at <= length
      match = pattern.match(self, at)
      break if match.nil?
      last_match = match
      item = match.size > 1 ? match.captures : match[0]
      if block.nil?
        found.push(item)
      else
        $~ = match
        block.call(item)
      end
      at = match.end(0) == match.begin(0) ? match.end(0) + 1 : match.end(0)
    end
    $~ = last_match
    block.nil? ? found : self
  end

  def sub(pattern, *replacement, &block)
    __substitute__(pattern, replacement, block, false)
  end

  def gsub(pattern, *replacement, &block)
    if replacement.empty? && block.nil?
      return to_enum(:gsub, pattern)
    end
    __substitute__(pattern, replacement, block, true)
  end

  def sub!(pattern, *replacement, &block)
    __modify__
    result = __substitute__(pattern, replacement, block, false, true)
    $~.nil? ? nil : replace(result)
  end

  def gsub!(pattern, *replacement, &block)
    __modify__
    return to_enum(:gsub!, pattern) if replacement.empty? && block.nil?
    matched = false
    result = __substitute__(pattern, replacement, block, true, true) { matched = true }
    matched ? replace(result) : nil
  end

  # One engine for all four: each match is replaced by a String (with `\1`,
  # `\0`, `\k<name>`, `` \` `` and `\'` expanded), a Hash lookup of the
  # matched text, or the block's answer with `$~` set to the match.
  #
  # Matching and copying read a snapshot, so a block that changes the
  # receiver does not change the answer — and in a bang method it is CRuby's
  # `RuntimeError: string modified`. Measured.
  def __substitute__(pattern, replacement, block, global, bang = false, &on_match)
    if replacement.size > 1
      raise ArgumentError, "wrong number of arguments (given #{replacement.size + 1}, expected 1..2)"
    end
    if replacement.empty? && block.nil?
      raise ArgumentError, "wrong number of arguments (given 1, expected 2)"
    end
    unless pattern.is_a?(Regexp)
      pattern = Regexp.new(Regexp.escape(String.__coerce__(pattern)))
    end
    hash = replacement.size == 1 && replacement[0].is_a?(Hash) ? replacement[0] : nil
    template = replacement.size == 1 && hash.nil? ? String.__coerce__(replacement[0]) : nil
    source = dup
    size = source.length
    out = String.new(encoding: encoding)
    at = 0
    copied = 0
    last_match = nil
    while at <= size
      match = pattern.match(source, at)
      break if match.nil?
      last_match = match
      on_match&.call
      out << source[copied, match.begin(0) - copied]
      piece =
        if hash
          hash[match[0]].to_s
        elsif template
          __expand_template__(template, match)
        else
          $~ = match
          answer = block.call(match[0]).to_s
          raise RuntimeError, "string modified" if bang && self != source
          answer
        end
      out << piece
      copied = match.end(0)
      if match.end(0) == match.begin(0)
        out << source[match.end(0), 1] if match.end(0) < size
        copied = match.end(0) + 1
        at = match.end(0) + 1
      else
        at = match.end(0)
      end
      break unless global
    end
    out << source[copied..] if copied <= size
    $~ = last_match
    out
  end

  def __expand_template__(template, match)
    out = String.new(encoding: template.encoding)
    i = 0
    size = template.length
    while i < size
      c = template[i]
      if c == "\\" && i + 1 < size
        n = template[i + 1]
        case n
        when "0", "&" then out << match[0]; i += 2; next
        when "`" then out << match.pre_match; i += 2; next
        when "'" then out << match.post_match; i += 2; next
        when "\\" then out << "\\"; i += 2; next
        when "+"
          last = (1...match.size).reverse_each.find { |group| !match[group].nil? }
          out << (last.nil? ? "" : match[last])
          i += 2
          next
        when "k"
          close = template.index(">", i)
          if template[i + 2] == "<" && close
            out << (match[template[i + 3...close]] || "")
            i = close + 1
            next
          end
        else
          if n >= "1" && n <= "9"
            out << (match[n.ord - 48] || "")
            i += 2
            next
          end
        end
      end
      out << c
      i += 1
    end
    out
  end
end

class String
  # --- character sets: tr, delete, squeeze, count ---------------------------

  # A `tr`-style set as a list of characters and whether it is negated:
  # ranges `a-z`, a leading `^` (when there is more after it), and `\` escaping
  # the next character. A descending range is CRuby's ArgumentError.
  def self.__char_set__(spec)
    spec = String.__coerce__(spec)
    raise ArgumentError, "invalid byte sequence in #{spec.encoding}" unless spec.valid_encoding?
    chars = spec.chars
    negated = chars.size > 1 && chars[0] == "^"
    chars = chars.drop(1) if negated
    out = []
    i = 0
    while i < chars.size
      c = chars[i]
      if c == "\\" && i + 1 < chars.size
        out.push(chars[i + 1])
        i += 2
      elsif i + 2 < chars.size && chars[i + 1] == "-"
        first = c.ord
        last = chars[i + 2].ord
        if first > last
          raise ArgumentError, "invalid range \"#{c}-#{chars[i + 2]}\" in string transliteration"
        end
        (first..last).each { |code| out.push(code < 0x80 ? Integer.__byte_string__(code).__force_encoding__(1) : Integer.__utf8__(code)) }
        i += 3
      else
        out.push(c)
        i += 1
      end
    end
    [out, negated]
  end

  # Whether `char` is in every one of `sets` — `delete("a-z", "^l")` is the
  # intersection.
  def self.__in_sets__(char, sets)
    sets.all? { |(chars, negated)| chars.include?(char) != negated }
  end

  def __sets__(specs)
    raise ArgumentError, "wrong number of arguments (given 0, expected 1+)" if specs.empty?
    specs.map { |spec| String.__char_set__(spec) }
  end

  def count(*specs)
    sets = __sets__(specs)
    n = 0
    each_char { |c| n += 1 if String.__in_sets__(c, sets) }
    n
  end

  def delete(*specs)
    sets = __sets__(specs)
    out = String.new(encoding: encoding)
    each_char { |c| out << c unless String.__in_sets__(c, sets) }
    out
  end

  def delete!(*specs)
    __bang__(delete(*specs))
  end

  def squeeze(*specs)
    sets = specs.empty? ? nil : __sets__(specs)
    out = String.new(encoding: encoding)
    previous = nil
    each_char do |c|
      next if c == previous && (sets.nil? || String.__in_sets__(c, sets))
      out << c
      previous = c
    end
    out
  end

  def squeeze!(*specs)
    __bang__(squeeze(*specs))
  end

  def tr(from, to)
    __translate__(from, to, false)
  end

  def tr_s(from, to)
    __translate__(from, to, true)
  end

  def tr!(from, to)
    __bang__(tr(from, to))
  end

  def tr_s!(from, to)
    __bang__(tr_s(from, to))
  end

  # The `to` list is padded with its last character; a negated `from` maps
  # every character outside it to that last character. `squeeze` collapses a
  # run that came from the same translation.
  def __translate__(from, to, squeeze)
    from_chars, negated = String.__char_set__(from)
    to_chars, = String.__char_set__(to)
    return dup if from_chars.empty?
    out = String.new(encoding: encoding)
    last_out = nil
    each_char do |c|
      hit = from_chars.include?(c) != negated
      unless hit
        out << c
        last_out = nil
        next
      end
      next if to_chars.empty?
      mapped = negated ? to_chars.last : (to_chars[from_chars.index(c)] || to_chars.last)
      next if squeeze && mapped == last_out
      out << mapped
      last_out = mapped
    end
    out
  end

  # --- successor ---------------------------------------------------------------

  def __alnum__(c)
    c.bytesize == 1 && ((c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9"))
  end

  # `succ` and `next` are the `__succ__` rules in `strings.rs`, a primitive
  # because `Range#each` and `upto` step through thousands of them.

  def succ!
    __bang__(succ) || self
  end

  def next!
    succ!
  end

  # Two single ASCII characters step by codepoint; two all-digit strings step
  # numerically; anything else steps by `succ` until it passes `max`'s length.
  def upto(max, exclusive = false, &block)
    max = String.__coerce__(max)
    return to_enum(:upto, max, exclusive) if block.nil?
    if length == 1 && max.length == 1 && ascii_only? && max.ascii_only?
      first = ord
      last = max.ord
      last -= 1 if exclusive
      (first..last).each { |code| block.call(Integer.__byte_string__(code).force_encoding(encoding)) }
      return self
    end
    if __all_digits__ && max.__all_digits__
      width = length
      (to_i..(exclusive ? max.to_i - 1 : max.to_i)).each do |n|
        text = n.to_s
        text = "0" * (width - text.length) + text if text.length < width
        block.call(text)
      end
      return self
    end
    current = dup
    while current.length <= max.length
      break if exclusive && current == max
      block.call(current)
      break if current == max
      current = current.succ
    end
    self
  end

  def __all_digits__
    !empty? && each_char.all? { |c| c >= "0" && c <= "9" }
  end

  # --- integers ----------------------------------------------------------------

  def to_i(base = 10)
    base = Integer.__index__(base)
    raise ArgumentError, "invalid radix #{base}" if base < 0 || base == 1 || base > 36
    __parse_integer__(base, false)
  end

  def hex
    __parse_integer__(16, false)
  end

  # Base 8, but any of Ruby's prefixes decides instead: `"0x1f".oct` is 31.
  def oct
    __parse_integer__(8, true)
  end

  # Leading whitespace, a sign, a base prefix where the base allows one, then
  # digits with single underscores between them; parsing stops at the first
  # character that is not one. Base 0 is chosen by the prefix.
  def __parse_integer__(base, any_prefix)
    i = 0
    i += 1 while i < bytesize && __strippable__(__getbyte__(i)) && __getbyte__(i) != 0
    negative = false
    if i < bytesize && (__getbyte__(i) == 0x2d || __getbyte__(i) == 0x2b)
      negative = __getbyte__(i) == 0x2d
      i += 1
    end
    if i + 1 < bytesize && __getbyte__(i) == 0x30
      prefixed =
        case __getbyte__(i + 1) | 0x20
        when 0x78 then 16
        when 0x62 then 2
        when 0x6f then 8
        when 0x64 then 10
        end
      if prefixed && (base == 0 || any_prefix || base == prefixed)
        base = prefixed
        i += 2
      elsif base == 0 || (any_prefix && base == 8)
        base = 8
      end
    end
    base = 10 if base == 0
    value = 0
    digits = 0
    previous_underscore = false
    while i < bytesize
      byte = __getbyte__(i)
      if byte == 0x5f
        break if previous_underscore || digits == 0
        previous_underscore = true
        i += 1
        next
      end
      digit =
        if byte >= 0x30 && byte <= 0x39 then byte - 0x30
        elsif byte >= 0x61 && byte <= 0x7a then byte - 0x61 + 10
        elsif byte >= 0x41 && byte <= 0x5a then byte - 0x41 + 10
        end
      break if digit.nil? || digit >= base
      value = value * base + digit
      digits += 1
      previous_underscore = false
      i += 1
    end
    negative ? -value : value
  end

  def chr
    empty? ? String.new(encoding: encoding) : self[0]
  end

  # `Kernel#Integer`'s parse: the whole String, or nil. Whitespace around it,
  # a sign, a prefix where the base allows one, and digits with single
  # underscores between them — nothing else, not even a NUL. Measured.
  def __strict_integer__(base)
    n = bytesize
    i = 0
    i += 1 while i < n && __strippable__(__getbyte__(i)) && __getbyte__(i) != 0
    negative = false
    if i < n && (__getbyte__(i) == 0x2d || __getbyte__(i) == 0x2b)
      negative = __getbyte__(i) == 0x2d
      i += 1
    end
    # Whether the last thing read was a digit: an underscore needs one on
    # each side.
    after_digit = false
    if i + 1 < n && __getbyte__(i) == 0x30
      prefixed =
        case __getbyte__(i + 1) | 0x20
        when 0x78 then 16
        when 0x62 then 2
        when 0x6f then 8
        when 0x64 then 10
        end
      if prefixed && (base == 0 || base == prefixed)
        base = prefixed
        i += 2
      elsif base == 0
        # A bare leading zero is octal, and is a digit itself.
        base = 8
        i += 1
        after_digit = true
      end
    end
    base = 10 if base == 0
    value = 0
    digits = after_digit ? 1 : 0
    while i < n
      byte = __getbyte__(i)
      if byte == 0x5f
        return nil unless after_digit
        after_digit = false
        i += 1
        next
      end
      digit =
        if byte >= 0x30 && byte <= 0x39 then byte - 0x30
        elsif byte >= 0x61 && byte <= 0x7a then byte - 0x61 + 10
        elsif byte >= 0x41 && byte <= 0x5a then byte - 0x41 + 10
        end
      break if digit.nil?
      return nil if digit >= base
      value = value * base + digit
      digits += 1
      after_digit = true
      i += 1
    end
    return nil if digits == 0 || !after_digit
    i += 1 while i < n && __strippable__(__getbyte__(i)) && __getbyte__(i) != 0
    return nil unless i == n
    negative ? -value : value
  end
end

# `format`, `sprintf` and `String#%` (#19): CRuby's `sprintf.c`, in Ruby. A
# directive is `%`, flags (`-+ 0#`), a width, a precision (either may be `*`),
# and a conversion; arguments are taken in order, by `%1$` position, or by
# `%<name>` / `%{name}` from one Hash, and mixing the styles is refused.
# Float digits come from `Float#__format__`. Every message is measured.
class String
  def %(args)
    String.__format__(self, args.is_a?(Array) ? args : [args])
  end

  class Formatter
    def initialize(format, args)
      @format = String.__coerce__(format)
      @args = args
      @next = 0
      @numbered = false
      @unnumbered = false
      @named = false
    end

    def run
      out = String.new(encoding: @format.encoding)
      chars = @format.chars
      i = 0
      while i < chars.size
        c = chars[i]
        if c != "%"
          out << c
          i += 1
          next
        end
        i = directive(chars, i + 1, out)
      end
      # CRuby's check, under `-w` only: positional arguments left over, unless
      # the one argument is the Hash a `%{name}` would have read.
      if !@numbered && !@named && @next < @args.size && !(@args.size == 1 && @args[0].is_a?(Hash))
        __warning__("too many arguments for format string", true)
      end
      out
    end

    def take(position = nil)
      if position
        raise ArgumentError, "unnumbered(#{@next}) mixed with numbered" if @unnumbered
        @numbered = true
        raise ArgumentError, "invalid index - #{position}$" if position < 1
        raise ArgumentError, "too few arguments" if position > @args.size
        return @args[position - 1]
      end
      raise ArgumentError, "unnumbered(#{@next + 1}) mixed with numbered" if @numbered
      @unnumbered = true
      raise ArgumentError, "too few arguments" if @next >= @args.size
      value = @args[@next]
      @next += 1
      value
    end

    def hash_argument
      @named = true
      unless @args.size == 1 && @args[0].is_a?(Hash)
        raise ArgumentError, "one hash required"
      end
      @args[0]
    end

    def named(name)
      hash = hash_argument
      key = name.to_sym
      unless hash.key?(key)
        # A default or a default proc answers, through `[]`. Measured.
        return hash[key] unless hash.default.nil? && hash.default_proc.nil?
        raise KeyError.new("key<#{name}> not found", receiver: hash, key: key)
      end
      hash[key]
    end

    # One directive starting after its `%`; answers where parsing resumes.
    def directive(chars, i, out)
      flags = []
      width = nil
      precision = nil
      precision_set = false
      value = nil
      have_value = false
      loop do
        raise ArgumentError, "incomplete format specifier; use %% (double %) instead" if i >= chars.size
        c = chars[i]
        case c
        when "-", "+", " ", "0", "#"
          flags.push(c)
          i += 1
        when "*"
          raise ArgumentError, "width given twice" unless width.nil?
          number, after = digits(chars, i + 1)
          if number && chars[after] == "$"
            width = Integer.__index__(take(number))
            i = after + 1
          else
            width = Integer.__index__(take)
            i += 1
          end
          if width < 0
            flags.push("-")
            width = -width
          end
        when "1", "2", "3", "4", "5", "6", "7", "8", "9"
          number, after = digits(chars, i)
          if chars[after] == "$"
            raise ArgumentError, "value given twice - #{number}$" if have_value
            value = take(number)
            have_value = true
          else
            raise ArgumentError, "width given twice" unless width.nil?
            width = number
          end
          i = chars[after] == "$" ? after + 1 : after
        when "."
          raise ArgumentError, "precision given twice" if precision_set
          precision_set = true
          i += 1
          if chars[i] == "*"
            precision = Integer.__index__(take)
            precision = nil if precision < 0
            i += 1
          else
            number, after = digits(chars, i)
            precision = number || 0
            raise ArgumentError, "precision too big" if precision > 2147483647
            i = after
          end
        when "<"
          close = (i...chars.size).find { |j| chars[j] == ">" }
          raise ArgumentError, "malformed name - unmatched parenthesis" if close.nil?
          value = named(chars[i + 1...close].join)
          have_value = true
          i = close + 1
        when "{"
          close = (i...chars.size).find { |j| chars[j] == "}" }
          raise ArgumentError, "malformed name - unmatched parenthesis" if close.nil?
          text = named(chars[i + 1...close].join).to_s
          text = text[0, precision] if precision
          out << pad(text, width, flags, false)
          return close + 1
        when "%"
          out << "%"
          return i + 1
        when "\n", "\0"
          out << "%" << c
          return i + 1
        else
          value = take unless have_value || !"diuxXobBcsfeEgGaAp".include?(c)
          raise ArgumentError, "malformed format string - %#{c}" unless "diuxXobBcsfeEgGaAp".include?(c)
          out << convert(c, value, flags, width, precision)
          return i + 1
        end
      end
    end

    def digits(chars, i)
      number = nil
      while i < chars.size && chars[i] >= "0" && chars[i] <= "9"
        number = (number || 0) * 10 + (chars[i].ord - 48)
        i += 1
      end
      [number, i]
    end

    def convert(conversion, value, flags, width, precision)
      case conversion
      when "d", "i", "u"
        integer(Kernel.__format_integer__(value), 10, false, flags, width, precision)
      when "x", "X"
        integer(Kernel.__format_integer__(value), 16, conversion == "X", flags, width, precision)
      when "o"
        integer(Kernel.__format_integer__(value), 8, false, flags, width, precision)
      when "b", "B"
        integer(Kernel.__format_integer__(value), 2, conversion == "B", flags, width, precision)
      when "f", "e", "E", "g", "G", "a", "A"
        float(Kernel.__format_float__(value), conversion, flags, width, precision)
      when "c"
        char = value.is_a?(Integer) ? value.chr(@format.encoding) : String.__coerce__(value)[0].to_s
        pad(char, width, flags, false)
      when "p"
        text = value.inspect
        text = text[0, precision] if precision
        pad(text, width, flags, false)
      else
        text = value.to_s
        text = text[0, precision] if precision
        pad(text, width, flags, false)
      end
    end

    def pad(text, width, flags, numeric)
      return text if width.nil? || text.length >= width
      fill = width - text.length
      if flags.include?("-")
        text + " " * fill
      elsif numeric && flags.include?("0")
        sign = text[0] == "-" || text[0] == "+" || text[0] == " " ? text[0] : ""
        rest = text[sign.length..]
        if rest.start_with?("0x", "0X", "0b", "0B")
          sign + rest[0, 2] + "0" * fill + rest[2..]
        else
          sign + "0" * fill + rest
        end
      else
        " " * fill + text
      end
    end

    # A negative number in base 2, 8 or 16 without `+` or ` ` is written as
    # its two's complement after `..`: `"%x" % -255` is `"..f01"`.
    def integer(n, base, upper, flags, width, precision)
      sign_flag = flags.include?("+") || flags.include?(" ")
      complement = n < 0 && base != 10 && !sign_flag
      if complement
        digits = abs_digits = n.abs.to_s(base)
        k = abs_digits.length + 1
        digits = (base**k + n).to_s(base)
        top = (base - 1).to_s(base)
        digits = digits[1..] while digits.length > 1 && digits[0] == top && digits[1] == top
        # A precision counts the `..`, and pads with the top digit.
        digits = top * (precision - 2 - digits.length) + digits if precision && precision - 2 > digits.length
        if width && flags.include?("0") && !flags.include?("-")
          digits = top * (width - 2 - digits.length) + digits if width - 2 > digits.length
        end
        digits = ".." + digits
      else
        # A zero at precision 0 is no digits at all. Measured.
        # except that `#` keeps octal's leading zero.
        digits = n == 0 && precision == 0 && !(base == 8 && flags.include?("#")) ? "" : n.abs.to_s(base)
      end
      digits = "0" * (precision - digits.length) + digits if precision && !complement && digits.length < precision
      if flags.include?("#") && n != 0
        prefix = { 16 => "0x", 8 => "0", 2 => "0b" }[base]
        # Octal's prefix is a leading zero, which a two's complement
        # (`..7651`) does not take. Measured.
        digits = prefix + digits if prefix && !(base == 8 && (digits.start_with?("0") || complement))
      end
      digits = digits.upcase if upper
      sign = n < 0 && !complement ? "-" : (flags.include?("+") ? "+" : (flags.include?(" ") ? " " : ""))
      pad(sign + digits, width, precision ? flags - ["0"] : flags, true)
    end

    def float(f, conversion, flags, width, precision)
      # NaN and the infinities are words, not digits, whatever the
      # conversion: no exponent, no precision, and padded with spaces even
      # under a `0` flag. Measured.
      unless f.finite?
        negative = !f.nan? && f < 0
        sign = negative ? "-" : (flags.include?("+") ? "+" : (flags.include?(" ") ? " " : ""))
        return pad(sign + (f.nan? ? "NaN" : "Inf"), width, flags - ["0"], true)
      end
      digits = f.__format__(conversion.ord, precision || 6, flags.include?("#"))
      negative = f < 0 || (f == 0.0 && f.to_s.start_with?("-"))
      sign = negative ? "-" : (flags.include?("+") ? "+" : (flags.include?(" ") ? " " : ""))
      pad(sign + digits, width, flags, true)
    end
  end

  def self.__format__(format, args)
    Formatter.new(format, args).run
  end
end

module Kernel
  def format(format, *args)
    String.__format__(format, args)
  end

  def sprintf(format, *args)
    String.__format__(format, args)
  end

  module_function :format, :sprintf

  # `%d`'s argument: an Integer, a Float truncated, or a String read as
  # `Integer()` reads one; nil is CRuby's TypeError.
  def self.__format_integer__(value)
    case value
    when Integer then value
    when Float then __float_to_i__(value)
    when String then value.__parse_integer__(0, false)
    when nil then raise TypeError, "can't convert nil into Integer"
    else
      raise TypeError, "can't convert #{value.class} into Integer" unless value.respond_to?(:to_int) || value.respond_to?(:to_i)
      value.respond_to?(:to_int) ? value.to_int : value.to_i
    end
  end

  # Truncation toward zero, as `%d` reads a Float: the digits at a precision
  # wide enough that rounding cannot carry into the integer part, cut at the
  # point. (`Float#to_i` is #18's.)
  def self.__float_to_i__(value)
    # NaN and the infinities raise `FloatDomainError` there.
    value.to_i
  end

  def self.__format_float__(value)
    case value
    when Float then value
    when Integer then value.to_f
    when nil then raise TypeError, "can't convert nil into Float"
    else
      raise TypeError, "can't convert #{value.class} into Float" unless value.respond_to?(:to_f)
      value.to_f
    end
  end
end

class String
  # `str.match(pattern)` is `pattern.match(str)`: a send, so a Regexp whose
  # `match` is its own is the one that answers (#203). What is not a Regexp is
  # made one first, from a String or what converts to one.
  def match(pattern, *rest, &block)
    __pattern__(pattern).match(self, *rest, &block)
  end

  # `match?` asks the engine itself, override or not. Measured.
  alias __match_p__ match?
  def match?(pattern, *rest)
    __match_p__(__pattern__(pattern), *rest)
  end

  def __pattern__(pattern)
    return pattern if pattern.is_a?(Regexp)
    source = pattern.is_a?(String) ? pattern : (pattern.respond_to?(:to_str) ? pattern.to_str : nil)
    unless source.is_a?(String)
      raise TypeError, "wrong argument type " + pattern.class.to_s + " (expected Regexp)"
    end
    Regexp.new(source)
  end

  # `=~` matches a Regexp itself, refuses a String, and asks anything else:
  # `str =~ obj` is `obj =~ str`. Measured.
  alias __match_op__ =~
  def =~(other)
    return __match_op__(other) if other.is_a?(Regexp) || other.is_a?(String)
    other =~ self
  end
end

class String
  # Two Strings are compared by the primitive, which sends anything else
  # here. It is converted with `to_str` if it has one, and otherwise asked —
  # `other <=> self` — with its answer turned round. Measured.
  def __cmp_other__(other)
    if other.respond_to?(:to_str)
      converted = other.to_str
      return converted.is_a?(String) ? self <=> converted : nil
    end
    return nil unless other.respond_to?(:<=>)
    order = other <=> self
    return nil if order.nil?
    order > 0 ? -1 : (order < 0 ? 1 : 0)
  end
end
