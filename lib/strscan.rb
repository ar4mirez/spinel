# `strscan`: `StringScanner` (#57), at the version CRuby 4.0.6 ships
# (3.1.6). CRuby's is `ext/strscan/strscan.c`; this follows it function by
# function in Ruby, over `Regexp#__match_at__`, which matches at one byte or
# searches from it and answers byte offsets without touching `$~`.
#
# Positions are bytes, as the C extension's are. Without `fixed_anchor:` a
# pattern sees the rest of the String as if it were the whole of it — `\A`
# and `^` hold at the scan pointer, and nothing before it is visible — so it
# is matched against that slice. With `fixed_anchor: true` it is matched in
# the whole String, from the pointer.
#
# ponytail: the slice is a copy each time, which makes a long scan quadratic
# in the String's length. A primitive that treats a byte offset as the start
# of the text is the upgrade.

class StringScanner
  Version = "3.1.6"
  Id = "$Id$"

  class Error < StandardError
  end

  def self.must_C_version = self

  def initialize(string, dup = false, fixed_anchor: false)
    @string = __string__(string)
    @fixed_anchor = fixed_anchor ? true : false
    @curr = 0
    __clear_match__
  end

  def initialize_copy(other)
    @string = other.string
    @fixed_anchor = other.fixed_anchor?
    @curr = other.pos
    @prev, @last, @matched, @names = other.__match_state__
  end

  def __match_state__ = [@prev, @last && @last.dup, @matched, @names]

  def __string__(value)
    return value if String === value
    raise TypeError, "no implicit conversion of #{value.nil? ? "nil" : value.class} into String" unless value.respond_to?(:to_str)
    value.to_str
  end

  def __clear_match__
    @matched = false
    @last = nil
    @prev = nil
    @names = nil
  end

  def __check__
    raise ArgumentError, "uninitialized StringScanner object" if @string.nil?
  end

  def fixed_anchor? = @fixed_anchor

  attr_reader :string

  def string=(string)
    @string = __string__(string)
    @curr = 0
    __clear_match__
    string
  end

  def concat(string)
    __check__
    @string << __string__(string)
    self
  end
  alias << concat

  def pos
    __check__
    @curr
  end
  alias pointer pos

  def pos=(position)
    __check__
    position = Integer.__index__(position)
    position += @string.bytesize if position < 0
    raise RangeError, "index out of range" if position < 0 || position > @string.bytesize
    @curr = position
  end
  alias pointer= pos=

  def charpos
    __check__
    @string.byteslice(0, @curr).length
  end

  def reset
    __check__
    @curr = 0
    __clear_match__
    self
  end

  def terminate
    __check__
    @curr = @string.bytesize
    __clear_match__
    self
  end

  def unscan
    __check__
    raise Error, "unscan error: not scanned yet" unless @matched
    @curr = @prev
    __clear_match__
    self
  end

  def eos?
    __check__
    @curr >= @string.bytesize
  end

  def rest?
    !eos?
  end

  def rest
    __check__
    @string.byteslice(@curr, @string.bytesize - @curr) || "".dup
  end

  def rest_size
    __check__
    @string.bytesize - @curr
  end

  def beginning_of_line?
    __check__
    @curr == 0 || @string.getbyte(@curr - 1) == 10
  end
  alias bol? beginning_of_line?

  # --- matching ----------------------------------------------------------------

  # CRuby's `strscan_do_scan`: match `pattern` at the pointer, or with
  # `headonly` false search from it; with `succeed` move past the match; and
  # answer the String or the length `getstr` asks for.
  def __scan__(pattern, succeed, getstr, headonly)
    __check__
    __clear_match__
    return nil if @curr > @string.bytesize
    offsets = if String === pattern
                __string_match__(pattern, headonly)
              else
                raise TypeError, "wrong argument type #{pattern.class} (expected Regexp)" unless Regexp === pattern
                __regexp_match__(pattern, headonly)
              end
    return nil if offsets.nil?
    @matched = true
    @prev = @curr
    @last = offsets
    @names = Regexp === pattern ? pattern.names : []
    finish = offsets[1]
    length = headonly ? finish - offsets[0] : finish - @prev
    @curr = finish if succeed
    return length unless getstr
    @string.byteslice(headonly ? offsets[0] : @prev, length)
  end

  def __regexp_match__(pattern, headonly)
    if @fixed_anchor
      pattern.__match_at__(@string, @curr, headonly)
    else
      offsets = pattern.__match_at__(rest, 0, headonly)
      offsets&.map { |at| at.nil? ? nil : at + @curr }
    end
  end

  # A String pattern is its bytes: at the pointer, or the first place after it.
  def __string_match__(pattern, headonly)
    rest = self.rest
    unless rest.encoding == pattern.encoding || (rest.ascii_only? && pattern.ascii_only?) || Encoding.compatible?(rest, pattern)
      raise Encoding::CompatibilityError, "incompatible encoding: #{pattern.encoding} and #{rest.encoding}"
    end
    if headonly
      return nil unless rest.b.start_with?(pattern.b)
      [@curr, @curr + pattern.bytesize]
    else
      found = rest.b.index(pattern.b)
      found.nil? ? nil : [@curr + found, @curr + found + pattern.bytesize]
    end
  end

  def scan(pattern) = __scan__(pattern, true, true, true)
  def match?(pattern) = __scan__(pattern, false, false, true)
  def skip(pattern) = __scan__(pattern, true, false, true)
  def check(pattern) = __scan__(pattern, false, true, true)
  def scan_full(pattern, advance, string) = __scan__(pattern, advance, string, true)
  def scan_until(pattern) = __scan__(pattern, true, true, false)
  def exist?(pattern) = __scan__(pattern, false, false, false)
  def skip_until(pattern) = __scan__(pattern, true, false, false)
  def check_until(pattern) = __scan__(pattern, false, true, false)
  def search_full(pattern, advance, string) = __scan__(pattern, advance, string, false)

  # One character in the String's encoding.
  def getch
    __check__
    __clear_match__
    return nil if eos?
    char = rest[0]
    length = char.bytesize
    @prev = @curr
    @last = [@curr, @curr + length]
    @names = []
    @matched = true
    @curr += length
    @string.byteslice(@prev, length)
  end

  def get_byte
    __check__
    __clear_match__
    return nil if eos?
    @prev = @curr
    @last = [@curr, @curr + 1]
    @names = []
    @matched = true
    @curr += 1
    @string.byteslice(@prev, 1)
  end

  def scan_byte
    __check__
    __clear_match__
    return nil if eos?
    byte = @string.getbyte(@curr)
    @prev = @curr
    @last = [@curr, @curr + 1]
    @names = []
    @matched = true
    @curr += 1
    byte
  end

  def peek_byte
    __check__
    eos? ? nil : @string.getbyte(@curr)
  end

  def peek(length)
    __check__
    length = IO.__c_long__(length)
    raise ArgumentError, "negative string size (or size too big)" if length < 0
    return "".dup if eos?
    @string.byteslice(@curr, length)
  end

  # `scan_integer(base: 10)`: an optionally signed run of digits in the base,
  # `0x` allowed for 16, as an Integer; nil when there is none.
  def scan_integer(base: 10)
    __check__
    unless rest.encoding.ascii_compatible?
      raise Encoding::CompatibilityError, "ASCII incompatible encoding: #{rest.encoding}"
    end
    pattern = case base
              when 10 then /[+-]?\d+/
              when 16 then /[+-]?(?:0x)?[0-9a-fA-F]+/
              else raise ArgumentError, "Unsupported integer base: #{base.inspect}, expected 10 or 16"
              end
    digits = scan(pattern)
    return nil if digits.nil?
    Integer(digits.delete_prefix("0x").sub(/\A([+-])0x/, '\1'), base)
  end

  # --- what the last match was ---------------------------------------------------

  def matched?
    __check__
    @matched
  end

  def matched
    __check__
    return nil unless @matched
    @string.byteslice(@last[0], @last[1] - @last[0])
  end

  def matched_size
    __check__
    return nil unless @matched
    @last[1] - @last[0]
  end

  def pre_match
    __check__
    return nil unless @matched
    @string.byteslice(0, @last[0])
  end

  def post_match
    __check__
    return nil unless @matched
    @string.byteslice(@last[1], @string.bytesize - @last[1])
  end

  def size
    __check__
    return nil unless @matched
    @last.size / 2
  end

  def [](index)
    __check__
    return nil unless @matched
    group = case index
            when Symbol, String
              name = index.to_s
              position = @names.index(name)
              raise IndexError, "undefined group name reference: #{name}" if position.nil?
              position + 1
            else
              Integer.__index__(index)
            end
    count = @last.size / 2
    group += count if group < 0
    return nil if group < 0 || group >= count
    start = @last[group * 2]
    finish = @last[group * 2 + 1]
    return nil if start.nil?
    @string.byteslice(start, finish - start)
  end

  def captures
    __check__
    return nil unless @matched
    (1...(@last.size / 2)).map { |group| self[group] }
  end

  def values_at(*indexes)
    __check__
    return nil unless @matched
    indexes.map { |index| self[index] }
  end

  def named_captures
    __check__
    return {} unless @matched
    @names.each_with_index.to_h { |name, position| [name, self[position + 1]] }
  end

  # CRuby's `inspect`: where the pointer is, five bytes either side of it,
  # and `fin` at the end.
  def inspect
    return "#<#{self.class} (uninitialized)>" if @string.nil?
    return "#<#{self.class} fin>" if eos?
    after = rest_size > 5 ? @string.byteslice(@curr, 5) + "..." : rest
    return "#<#{self.class} #{@curr}/#{@string.bytesize} @ #{after.dump}>" if @curr == 0
    before = @curr > 5 ? "..." + @string.byteslice(@curr - 5, 5) : @string.byteslice(0, @curr)
    "#<#{self.class} #{@curr}/#{@string.bytesize} #{before.dump} @ #{after.dump}>"
  end
end

ScanError = StringScanner::Error
