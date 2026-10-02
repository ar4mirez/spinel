# `Array#pack`, `String#unpack` and `#unpack1` (#19): CRuby's `pack.c`, in
# Ruby. A format is directives, each a letter with an optional `_`/`!`
# (native size), `<`/`>` (endianness) and a count (a number or `*`);
# whitespace is ignored and `#` comments to the end of the line. Integers wrap
# two's-complement to their width; floats go through `Float#__bits__`. The
# pointer directives `p` and `P` are refused: there are no raw pointers to
# give. Every message here was measured on ruby 4.0.7.
module Pack
  # Integer directives: [bytes, signed, endianness or nil for native].
  # This is a little-endian 64-bit platform, as every target Spinel builds is.
  INTEGERS = {
    "C" => [1, false, :little], "c" => [1, true, :little],
    "S" => [2, false, :little], "s" => [2, true, :little],
    "L" => [4, false, :little], "l" => [4, true, :little],
    "Q" => [8, false, :little], "q" => [8, true, :little],
    "J" => [8, false, :little], "j" => [8, true, :little],
    "I" => [4, false, :little], "i" => [4, true, :little],
    "N" => [4, false, :big], "n" => [2, false, :big],
    "V" => [4, false, :little], "v" => [2, false, :little],
  }.freeze

  # Native sizes under `_` or `!`: `L!` is a C long, eight bytes here.
  NATIVE = { "S" => 2, "s" => 2, "I" => 4, "i" => 4, "L" => 8, "l" => 8,
             "Q" => 8, "q" => 8, "J" => 8, "j" => 8 }.freeze

  # Float directives: [bytes, endianness].
  FLOATS = {
    "D" => [8, :little], "d" => [8, :little], "F" => [4, :little], "f" => [4, :little],
    "E" => [8, :little], "e" => [4, :little], "G" => [8, :big], "g" => [4, :big],
  }.freeze

  KNOWN = "CcSsLlQqJjIiNnVvUwDdFfEeGgaAZBbHhmMuxX@pP"

  # The format as [letter, count, native, endianness], count being an
  # Integer, `:star`, or nil when none was written.
  def self.parse(format)
    format = String.__coerce__(format)
    directives = []
    chars = format.chars
    i = 0
    while i < chars.size
      c = chars[i]
      i += 1
      next if c == " " || c == "\t" || c == "\n" || c == "\v" || c == "\f" || c == "\r"
      if c == "#"
        i += 1 while i < chars.size && chars[i] != "\n"
        next
      end
      unless KNOWN.include?(c)
        shown = c == "\0" ? "\x00" : c
        raise ArgumentError, "unknown #{yield} directive '#{shown}' in '#{format.inspect[1...-1]}'"
      end
      native = false
      endian = nil
      while i < chars.size && "_!<>".include?(chars[i])
        modifier = chars[i]
        if modifier == "_" || modifier == "!"
          unless NATIVE.key?(c)
            raise ArgumentError, "'#{modifier}' allowed only after types sSiIlLqQjJ"
          end
          native = true
        else
          unless INTEGERS.key?(c) && !"NnVv".include?(c)
            raise ArgumentError, "'#{modifier}' allowed only after types sSiIlLqQjJ"
          end
          endian = modifier == "<" ? :little : :big
        end
        i += 1
      end
      count = nil
      if i < chars.size && chars[i] == "*"
        count = :star
        i += 1
      elsif i < chars.size && chars[i] >= "0" && chars[i] <= "9"
        count = 0
        while i < chars.size && chars[i] >= "0" && chars[i] <= "9"
          count = count * 10 + chars[i].ord - 48
          i += 1
        end
      end
      directives.push([c, count, native, endian])
    end
    directives
  end

  def self.width(letter, native)
    native ? NATIVE[letter] : INTEGERS[letter][0]
  end

  # `value` as `bytes` little- or big-endian bytes, two's complement.
  def self.int_bytes(value, bytes, endian)
    value = value & ((1 << (bytes * 8)) - 1)
    out = []
    bytes.times do
      out.push(value & 0xff)
      value = value >> 8
    end
    out.reverse! if endian == :big
    out
  end

  def self.bytes_to_string(bytes)
    out = String.new(encoding: Encoding::BINARY)
    bytes.each { |b| out.__splice__(out.bytesize, 0, Integer.__byte_string__(b)) }
    out
  end

  def self.integer_arg(value)
    case value
    when Integer then value
    when Float then Kernel.__float_to_i__(value)
    when nil then raise TypeError, "no implicit conversion of nil into Integer"
    else
      unless value.respond_to?(:to_int)
        raise TypeError, "no implicit conversion of #{value.class} into Integer"
      end
      value.to_int
    end
  end

  def self.float_arg(value)
    case value
    when Float then value
    when Integer then value.to_f
    when nil then raise TypeError, "can't convert nil into Float"
    when String then raise TypeError, "can't convert String into Float"
    else
      raise TypeError, "can't convert #{value.class} into Float" unless value.respond_to?(:to_f)
      value.to_f
    end
  end

  BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

  def self.base64(bytes, line)
    out = +""
    groups = []
    bytes.each_slice(3) { |group| groups.push(group) }
    per_line = line.nil? ? 15 : line / 3
    groups.each_with_index do |group, index|
      n = (group[0] << 16) | ((group[1] || 0) << 8) | (group[2] || 0)
      out << BASE64[(n >> 18) & 63] << BASE64[(n >> 12) & 63]
      out << (group.size > 1 ? BASE64[(n >> 6) & 63] : "=")
      out << (group.size > 2 ? BASE64[n & 63] : "=")
      out << "\n" if per_line > 0 && (index + 1) % per_line == 0
    end
    out << "\n" if per_line > 0 && groups.size % per_line != 0
    out
  end

  # CRuby's `qpencode`: bytes past `~`, controls other than newline and tab,
  # and `=` are escaped; a space or tab before a newline gets a soft break;
  # a line longer than `limit` is broken; and a last line without a newline
  # ends in a soft break.
  def self.qprint(bytes, line)
    out = +""
    n = 0
    previous = nil
    limit = line.nil? || line <= 1 ? 72 : line
    bytes.each do |b|
      if b > 126 || (b < 32 && b != 0x0a && b != 0x09) || b == 0x3d
        out << "=" << Integer.__hex__(b, 2)
        n += 3
        previous = nil
      elsif b == 0x0a
        out << "=\n" if previous == 0x20 || previous == 0x09
        out << "\n"
        n = 0
        previous = b
      else
        out << Integer.__byte_string__(b).force_encoding(Encoding::US_ASCII)
        n += 1
        previous = b
      end
      if n > limit
        out << "=\n"
        n = 0
      end
    end
    out << "=\n" if n > 0
    out
  end

  def self.uuencode(bytes, line)
    out = +""
    per_line = line.nil? || line < 3 ? 45 : (line / 3) * 3
    per_line = 45 if per_line > 45
    bytes.each_slice(per_line) do |chunk|
      out << (32 + chunk.size).chr
      chunk.each_slice(3) do |group|
        n = (group[0] << 16) | ((group[1] || 0) << 8) | (group[2] || 0)
        4.times do |k|
          six = (n >> (18 - 6 * k)) & 63
          out << (six == 0 ? "`" : (32 + six).chr)
        end
      end
      out << "\n"
    end
    out
  end

  def self.ber(value)
    raise ArgumentError, "can't compress negative numbers" if value < 0
    groups = [value & 0x7f]
    value = value >> 7
    while value > 0
      groups.unshift((value & 0x7f) | 0x80)
      value = value >> 7
    end
    groups
  end
end

class Array
  def pack(format, buffer: nil)
    directives = Pack.parse(format) { "pack" }
    unless buffer.nil?
      raise TypeError, "buffer must be String, not #{buffer.class}" unless buffer.is_a?(String)
      Kernel.__check_frozen__(buffer)
    end
    out = buffer.nil? ? String.new(encoding: Encoding::BINARY) : buffer
    first = directives.first
    encoding_index =
      if first.nil? then 2
      elsif first[0] == "U" then 1
      elsif "mMu".include?(first[0]) then 2
      else 0
      end
    item = 0
    take = lambda do
      raise ArgumentError, "too few arguments" if item >= size
      value = self[item]
      item += 1
      value
    end
    directives.each do |letter, count, native, endian|
      case letter
      when "C", "c", "S", "s", "L", "l", "Q", "q", "J", "j", "I", "i", "N", "n", "V", "v"
        bytes = Pack.width(letter, native)
        order = endian || Pack::INTEGERS[letter][2]
        times = count == :star ? size - item : (count || 1)
        times.times do
          out << Pack.bytes_to_string(Pack.int_bytes(Pack.integer_arg(take.call), bytes, order))
        end
      when "U"
        times = count == :star ? size - item : (count || 1)
        times.times do
          code = Pack.integer_arg(take.call)
          raise RangeError, "pack(U): value out of range" if code < 0
          out << Integer.__utf8__(code).b
        end
      when "w"
        times = count == :star ? size - item : (count || 1)
        times.times { out << Pack.bytes_to_string(Pack.ber(Pack.integer_arg(take.call))) }
      when "D", "d", "F", "f", "E", "e", "G", "g"
        bytes, order = Pack::FLOATS[letter]
        times = count == :star ? size - item : (count || 1)
        times.times do
          bits = Pack.float_arg(take.call).__bits__(bytes * 8)
          out << Pack.bytes_to_string(Pack.int_bytes(bits, bytes, order))
        end
      when "a", "A", "Z"
        value = take.call
        # nil packs as an empty string here, and only here. Measured.
        text = value.nil? ? "".b : String.__coerce__(value).b
        width =
          if count == :star then letter == "Z" ? text.bytesize + 1 : text.bytesize
          else count || 1
          end
        field = text.byteslice(0, width)
        pad = letter == "A" ? " " : "\0"
        field << pad.b * (width - field.bytesize) if field.bytesize < width
        out << field
      when "B", "b"
        bits = String.__coerce__(take.call)
        width = count == :star ? bits.length : (count || 1)
        # A count past the string pads with NUL bytes — CRuby's formula,
        # which for bits counts half a byte per missing bit.
        extra = width > bits.length ? (width - bits.length + 1) / 2 : 0
        width = bits.length if width > bits.length
        bytes = []
        (0...width).each_slice(8) do |slice|
          byte = 0
          slice.each_with_index do |at, k|
            on = bits.getbyte(at) & 1 == 1
            byte = byte | (letter == "B" ? 0x80 >> k : 1 << k) if on
          end
          bytes.push(byte)
        end
        bytes.concat([0] * extra)
        out << Pack.bytes_to_string(bytes)
      when "H", "h"
        hex = String.__coerce__(take.call)
        width = count == :star ? hex.length : (count || 1)
        extra = width > hex.length ? (width + 1) / 2 - (hex.length + 1) / 2 : 0
        width = hex.length if width > hex.length
        bytes = []
        (0...width).each_slice(2) do |slice|
          byte = 0
          slice.each_with_index do |at, k|
            c = hex.getbyte(at)
            nibble = c >= 0x61 ? (c - 0x61 + 10) & 15 : (c >= 0x41 ? (c - 0x41 + 10) & 15 : c & 15)
            byte = byte | (letter == "H" ? (k == 0 ? nibble << 4 : nibble) : (k == 0 ? nibble : nibble << 4))
          end
          bytes.push(byte)
        end
        bytes.concat([0] * extra)
        out << Pack.bytes_to_string(bytes)
      when "m"
        text = String.__coerce__(take.call)
        line = count == :star || count.nil? ? nil : count
        out << (count == 0 ? Pack.base64(text.bytes, 0) : Pack.base64(text.bytes, line.nil? ? nil : [line, 3].max)).b
      when "M"
        value = take.call
        text = value.is_a?(String) ? value : value.to_s
        out << Pack.qprint(text.bytes, count == :star ? nil : count).b
      when "u"
        text = String.__coerce__(take.call)
        out << Pack.uuencode(text.bytes, count == :star ? nil : count).b
      when "x"
        out << "\0".b * (count == :star ? 0 : (count || 1))
      when "X"
        back = count == :star ? 0 : (count || 1)
        raise ArgumentError, "X outside of string" if back > out.bytesize
        out.__splice__(out.bytesize - back, back, "")
      when "@"
        position = count == :star ? 0 : (count || 1)
        if position > out.bytesize
          out << "\0".b * (position - out.bytesize)
        else
          out.__splice__(position, out.bytesize - position, "")
        end
      when "p", "P"
        __needs_pointers__
      end
    end
    out.__force_encoding__(encoding_index) if buffer.nil?
    out
  end
end

class String
  def unpack(format, offset: 0, &block)
    results = __unpack__(format, offset)
    return results if block.nil?
    results.each(&block)
    nil
  end

  def unpack1(format, offset: 0)
    __unpack__(format, offset, true)[0]
  end

  def __unpack__(format, offset, single = false)
    directives = Pack.parse(format) { "unpack" }
    offset = Integer.__index__(offset)
    raise ArgumentError, "offset can't be negative" if offset < 0
    raise ArgumentError, "offset outside of string" if offset > bytesize
    bytes = __bytes__
    at = offset
    out = []
    directives.each do |letter, count, native, endian|
      break if single && !out.empty?
      case letter
      when "C", "c", "S", "s", "L", "l", "Q", "q", "J", "j", "I", "i", "N", "n", "V", "v"
        width = Pack.width(letter, native)
        signed = Pack::INTEGERS[letter][1]
        order = endian || Pack::INTEGERS[letter][2]
        times = count == :star ? (bytes.size - at) / width : (count || 1)
        times.times do
          if at + width > bytes.size
            out.push(nil) unless count == :star
            next
          end
          chunk = bytes[at, width]
          chunk = chunk.reverse if order == :big
          value = 0
          chunk.reverse_each { |b| value = (value << 8) | b }
          value -= 1 << (width * 8) if signed && value >= 1 << (width * 8 - 1)
          out.push(value)
          at += width
        end
      when "U"
        times = count == :star ? nil : (count || 1)
        n = 0
        while at < bytes.size && (times.nil? || n < times)
          lead = bytes[at]
          len = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4
          raise ArgumentError, "malformed UTF-8 character" if lead >= 0x80 && lead < 0xc0
          if at + len > bytes.size
            raise ArgumentError, "malformed UTF-8 character (expected #{len} bytes, given #{bytes.size - at} bytes)"
          end
          out.push(Integer.__utf8_decode__(bytes[at, len]))
          at += len
          n += 1
        end
      when "w"
        times = count == :star ? nil : (count || 1)
        n = 0
        while at < bytes.size && (times.nil? || n < times)
          value = 0
          loop do
            b = bytes[at]
            at += 1
            value = (value << 7) | (b & 0x7f)
            break if b < 0x80 || at >= bytes.size
          end
          out.push(value)
          n += 1
        end
      when "D", "d", "F", "f", "E", "e", "G", "g"
        width, order = Pack::FLOATS[letter]
        times = count == :star ? (bytes.size - at) / width : (count || 1)
        times.times do
          if at + width > bytes.size
            out.push(nil) unless count == :star
            next
          end
          chunk = bytes[at, width]
          chunk = chunk.reverse if order == :big
          bits = 0
          chunk.reverse_each { |b| bits = (bits << 8) | b }
          out.push(bits.__float_from_bits__(width * 8))
          at += width
        end
      when "a", "A", "Z"
        width = count == :star ? bytes.size - at : (count || 1)
        width = bytes.size - at if width > bytes.size - at
        field = __byteslice__(at, width).b
        if letter == "Z"
          nul = field.__byte_index__("\0", 0)
          if nul
            field = field.__byteslice__(0, nul)
            width = nul + 1 if count == :star
          end
        elsif letter == "A"
          stop = field.bytesize
          stop -= 1 while stop > 0 && (field.getbyte(stop - 1) == 0x20 || field.getbyte(stop - 1) == 0)
          field = field.__byteslice__(0, stop)
        end
        out.push(field)
        at += width
      when "B", "b"
        width = count == :star ? (bytes.size - at) * 8 : (count || 1)
        width = (bytes.size - at) * 8 if width > (bytes.size - at) * 8
        text = String.new(encoding: Encoding::US_ASCII)
        width.times do |k|
          byte = bytes[at + k / 8]
          on = letter == "B" ? (byte >> (7 - k % 8)) & 1 : (byte >> (k % 8)) & 1
          text << (on == 1 ? "1" : "0")
        end
        out.push(text)
        at += (width + 7) / 8
      when "H", "h"
        width = count == :star ? (bytes.size - at) * 2 : (count || 1)
        width = (bytes.size - at) * 2 if width > (bytes.size - at) * 2
        text = String.new(encoding: Encoding::US_ASCII)
        width.times do |k|
          byte = bytes[at + k / 2]
          nibble = (letter == "H") == k.even? ? byte >> 4 : byte & 15
          text << "0123456789abcdef"[nibble]
        end
        out.push(text)
        at += (width + 1) / 2
      when "m"
        decoded, at = __unbase64__(bytes, at, count == 0)
        out.push(decoded)
      when "M"
        decoded = []
        while at < bytes.size
          b = bytes[at]
          if b == 0x3d
            if at + 1 < bytes.size && bytes[at + 1] == 0x0a
              at += 2
              next
            end
            if at + 2 < bytes.size && bytes[at + 1, 2].all? { |h| (h >= 0x30 && h <= 0x39) || (h | 0x20 >= 0x61 && h | 0x20 <= 0x66) }
              decoded.push(__byteslice__(at + 1, 2).to_i(16))
              at += 3
              next
            end
          end
          decoded.push(b)
          at += 1
        end
        out.push(Pack.bytes_to_string(decoded))
      when "u"
        decoded = []
        while at < bytes.size
          length = (bytes[at] - 32) & 63
          at += 1
          line = []
          while at < bytes.size && bytes[at] != 0x0a
            line.push((bytes[at] - 32) & 63)
            at += 1
          end
          at += 1
          # Each line decodes whole groups, then keeps the length its
          # first character announced.
          line_bytes = []
          line.each_slice(4) do |group|
            n = 0
            4.times { |k| n = (n << 6) | (group[k] || 0) }
            line_bytes.push((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff)
          end
          decoded.concat(line_bytes.first(length))
        end
        out.push(Pack.bytes_to_string(decoded))
      when "x"
        skip = count == :star ? bytes.size - at : (count || 1)
        raise ArgumentError, "x outside of string" if at + skip > bytes.size
        at += skip
      when "X"
        back = count == :star ? bytes.size - at : (count || 1)
        raise ArgumentError, "X outside of string" if back > at
        at -= back
      when "@"
        # No count is 0 here, where `pack` takes 1; `*` stays put.
        next if count == :star
        position = count || 0
        raise ArgumentError, "@ outside of string" if offset + position > bytes.size
        at = offset + position
      when "p", "P"
        __needs_pointers__
      end
    end
    out
  end

  def __unbase64__(bytes, at, strict)
    values = []
    index = {}
    Pack::BASE64.each_char.with_index { |c, i| index[c.ord] = i }
    padding = 0
    while at < bytes.size
      b = bytes[at]
      at += 1
      if b == 0x3d
        padding += 1
        next
      end
      value = index[b]
      if value.nil?
        raise ArgumentError, "invalid base64" if strict && b != 0x0a
        next
      end
      raise ArgumentError, "invalid base64" if strict && padding > 0
      values.push(value)
    end
    if strict && (values.size + padding) % 4 != 0
      raise ArgumentError, "invalid base64"
    end
    decoded = []
    values.each_slice(4) do |group|
      n = 0
      4.times { |k| n = (n << 6) | (group[k] || 0) }
      decoded.push((n >> 16) & 0xff)
      decoded.push((n >> 8) & 0xff) if group.size > 2
      decoded.push(n & 0xff) if group.size > 3
    end
    [Pack.bytes_to_string(decoded), at]
  end
end
