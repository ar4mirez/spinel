#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/src/jis_table.rs` (#378): how CRuby maps
# EUC-JP, Shift_JIS, Windows-31J and CP51932 (#419) to Unicode and back. The mapping is a table,
# not arithmetic, so it is measured through `String#encode` on a real Ruby
# rather than copied from a standard: what CRuby's `enc/trans/JIS` tables say
# is the answer, vendor quirks included.
#
#   ruby scripts/jis-table.rb --check      # CI: agree, or fail
#   ruby scripts/jis-table.rb --generate   # rewrite the table
#
# A character is a *cell*: its row and column, from 0, in a plane of 94
# columns. JIS X 0208 is EUC-JP's two-byte plane and Shift_JIS's; JIS X 0212 is
# EUC-JP's three-byte one, after 0x8F; Windows-31J's 120 rows are every
# two-byte Shift_JIS code, its vendor rows included; CP51932's 94 rows are EUC-JP's
# two-byte shape over Windows-31J's first 94, which is checked rather than
# tabled again. ASCII and the half-width katakana are arithmetic and are checked
# here, not tabled.
#
# Besides the planes, the table records what decoding does not determine:
# which cell CRuby writes for a character more than one cell decodes to, and
# the characters it writes as a cell that reads back as another. Everything
# else the Rust side does by hand — which bytes lead, which follow, what an
# invalid sequence reports, the direct EUC-JP and Shift_JIS converter, and
# CP50220 and CP50221 to and from CP51932 (`cp5022x.rs`) — is checked against
# CRuby here too, so a Ruby that changes any of it fails CI. What CP50220 makes
# of the half-width katakana, which it writes full-width, is tabled.

TABLE = File.expand_path("../crates/spinel-vm/src/jis_table.rs", __dir__)
CELLS = 94 * 94
ENCODINGS = %w[EUC-JP Shift_JIS Windows-31J CP51932].freeze

# Whether `encoding` has EUC-JP's shape: CP51932 is EUC-JP without 0x8F.
def euc?(encoding) = encoding == "EUC-JP" || encoding == "CP51932"


def decode(encoding, bytes)
  [:char, bytes.pack("C*").force_encoding(encoding).encode("UTF-8").ord]
rescue Encoding::UndefinedConversionError
  [:undefined]
rescue Encoding::InvalidByteSequenceError => e
  [e.incomplete_input? ? :incomplete : :invalid, e.error_bytes.bytesize, e.readagain_bytes&.bytesize || 0]
end

def code(encoding, bytes)
  outcome = decode(encoding, bytes)
  outcome[0] == :char ? outcome[1] : nil
end

def euc(cell, supplementary: false)
  bytes = [0xA1 + cell / 94, 0xA1 + cell % 94]
  supplementary ? [0x8F, *bytes] : bytes
end

# The Shift_JIS bytes of a cell in rows 0 to 119.
def sjis(cell)
  row, column = cell.divmod(94)
  lead = row / 2
  trail = (row % 2) * 94 + column
  [lead + (lead < 31 ? 0x81 : 0xC1), trail + (trail < 63 ? 0x40 : 0x41)]
end

# The cell, in its encoding's numbering, that `bytes` spell, or :kana with the
# byte for a half-width katakana.
def cell_of(encoding, bytes)
  if euc?(encoding)
    case bytes
    in [0x8E, b] then [:kana, b]
    in [0x8F, a, b] then CELLS + (a - 0xA1) * 94 + (b - 0xA1)
    in [a, b] then (a - 0xA1) * 94 + (b - 0xA1)
    end
  else
    case bytes
    in [b] then [:kana, b]
    in [lead, trail]
      row = (lead - (lead < 0xA0 ? 0x81 : 0xC1)) * 2
      trail -= trail < 0x7F ? 0x40 : 0x41
      row += 1 if trail >= 94
      row * 94 + trail % 94
    end
  end
end

def fail!(message)
  warn "jis-table: #{message}"
  exit 1
end

def expect(what, actual, expected)
  fail!("#{what}: CRuby says #{actual.inspect}, the Rust side assumes #{expected.inspect}") unless actual == expected
end

# What the Rust side's reader assumes about the bytes alone: for every one-,
# two- and (EUC-JP) three-byte prefix, a character, a sequence the input ends
# inside, or an invalid one with how many bytes are the error and how many are
# read again. A character may still be undefined; that is the planes' business.
def model(encoding, bytes)
  trail = ->(b) { (0xA1..0xFE).cover?(b) }
  first, second, third = bytes
  if euc?(encoding)
    return :char if first < 0x80
    return [:invalid, 1, 0] unless first == 0x8E || (first == 0x8F && encoding == "EUC-JP") || trail.(first)
    return [:incomplete, 1, 0] if second.nil?
    return [:invalid, 1, 1] unless trail.(second)
    return :char unless first == 0x8F
    return [:incomplete, 2, 0] if third.nil?
    trail.(third) ? :char : [:invalid, 2, 1]
  else
    return :char if first < 0x80 || (0xA1..0xDF).cover?(first)
    return [:invalid, 1, 0] unless (0x81..0x9F).cover?(first) || (0xE0..0xFC).cover?(first)
    return [:incomplete, 1, 0] if second.nil?
    (0x40..0x7E).cover?(second) || (0x80..0xFC).cover?(second) ? :char : [:invalid, 1, 1]
  end
end

def check_reader(encoding)
  prefixes = (0..0xFF).map { [_1] }
  until prefixes.empty?
    bytes = prefixes.shift
    outcome = decode(encoding, bytes)
    outcome = :char if outcome[0] == :char || outcome[0] == :undefined
    expect("#{encoding} reading #{bytes.map { format("%02X", _1) }.join}", outcome, model(encoding, bytes))
    prefixes.concat((0..0xFF).map { bytes + [_1] }) if outcome[0] == :incomplete
  end
end

def check_arithmetic(encoding)
  (0..0x7F).each do |b|
    expect("#{encoding} ASCII #{b}", code(encoding, [b]), b)
    expect("#{encoding} writing ASCII #{b}", b.chr.encode(encoding).bytes, [b])
  end
  (0xA1..0xDF).each do |b|
    bytes = euc?(encoding) ? [0x8E, b] : [b]
    expect("#{encoding} katakana #{b}", code(encoding, bytes), 0xFF61 + b - 0xA1)
    expect("#{encoding} writing katakana #{b}", (0xFF61 + b - 0xA1).chr("UTF-8").encode(encoding).bytes, bytes)
  end
  return unless euc?(encoding)
  (0xE0..0xFE).each { |b| expect("#{encoding} 8E #{b}", decode(encoding, [0x8E, b]), [:undefined]) }
end

# EUC-JP and Shift_JIS convert to each other directly, not through UTF-8, and
# by arithmetic: every JIS X 0208 cell, mapped or not, and the katakana. JIS X
# 0212, EUC-JP's undefined katakana bytes and Shift_JIS's rows past 94 have no
# counterpart.
def check_direct
  convert = lambda do |from, to, bytes|
    bytes.pack("C*").force_encoding(from).encode(to).bytes
  rescue Encoding::UndefinedConversionError
    :undefined
  end
  path = Encoding::Converter.search_convpath("EUC-JP", "Shift_JIS")
  expect("EUC-JP to Shift_JIS path", path, [[Encoding::EUC_JP, Encoding::Shift_JIS]])
  CELLS.times do |cell|
    expect("EUC-JP to Shift_JIS #{cell}", convert.("EUC-JP", "Shift_JIS", euc(cell)), sjis(cell))
    expect("Shift_JIS to EUC-JP #{cell}", convert.("Shift_JIS", "EUC-JP", sjis(cell)), euc(cell))
    expect("EUC-JP 0212 to Shift_JIS #{cell}", convert.("EUC-JP", "Shift_JIS", euc(cell, supplementary: true)), :undefined)
  end
  (94 * 94...120 * 94).each do |cell|
    expect("Shift_JIS to EUC-JP #{cell}", convert.("Shift_JIS", "EUC-JP", sjis(cell)), :undefined)
  end
  (0xA1..0xFE).each do |b|
    expect("EUC-JP 8E #{b} to Shift_JIS", convert.("EUC-JP", "Shift_JIS", [0x8E, b]), b <= 0xDF ? [b] : :undefined)
  end
  (0xA1..0xDF).each { |b| expect("Shift_JIS #{b} to EUC-JP", convert.("Shift_JIS", "EUC-JP", [b]), [0x8E, b]) }
end

# CP50220 and CP50221 convert to and from CP51932 alone, by hand rather than by
# table: what `cp5022x.rs` does is checked here. Both decode alike.
CP5022X = %w[CP50220 CP50221].freeze

# What G0 holds after each escape sequence, SO or SI, which the decoder reads
# by; the encoder writes only ASCII, JIS X 0208 1983 and the katakana.
CP5022X_SWITCHES = {
  [0x1B, 0x28, 0x42] => :ascii, [0x1B, 0x28, 0x4A] => :ascii, [0x1B, 0x28, 0x49] => :kana,
  [0x1B, 0x24, 0x40] => :jis1978, [0x1B, 0x24, 0x42] => :jis1983, [0x0E] => :kana, [0x0F] => :ascii
}.freeze

def cp5022x_decode(encoding, bytes)
  converter = Encoding::Converter.new(encoding, "CP51932")
  out = +""
  result = converter.primitive_convert(bytes.pack("C*").force_encoding(encoding), out, nil, nil)
  return [:char, out.bytes] if result == :finished
  info = converter.primitive_errinfo
  [result == :incomplete_input ? :incomplete : :invalid, info[3].bytesize, info[4].bytesize]
end

# The Rust side's decoder: one sequence read with G0 holding `g0`, as the
# CP51932 bytes it writes, none for a switch, or why it is none. A byte
# through 0x92 is itself in ASCII, and one in JIS X 0208 1978 reads the rows
# 1983 adds as invalid.
def cp5022x_model(g0, bytes)
  first, second, third = bytes
  if first == 0x1B
    return [:incomplete, 1, 0] if second.nil?
    return [:invalid, 1, 1] unless second == 0x24 || second == 0x28
    return [:incomplete, 2, 0] if third.nil?
    return CP5022X_SWITCHES.key?(bytes.first(3)) ? [:char, []] : [:invalid, 2, 1]
  end
  return [:char, []] if CP5022X_SWITCHES.key?([first])
  kana = (0xA1..0xDF).cover?(first)
  case g0
  when :ascii
    return [:char, [0x8E, first]] if kana
    first <= 0x92 ? [:char, [first]] : [:invalid, 1, 0]
  when :kana
    kana || (0x21..0x5F).cover?(first) ? [:char, [0x8E, first | 0x80]] : [:invalid, 1, 0]
  else
    rows = [0x21..0x28, 0x30..0x74]
    rows += [0x2D..0x2D, 0x79..0x7C] if g0 == :jis1983
    return [:invalid, 1, 0] unless rows.any? { _1.cover?(first) }
    return [:incomplete, 1, 0] if second.nil?
    (0x21..0x7E).cover?(second) ? [:char, [first | 0x80, second | 0x80]] : [:invalid, 1, 1]
  end
end

def check_cp5022x_decoder(encoding)
  states = { [] => :ascii, [0x0E, 0x0F] => :ascii, [0x1B, 0x24, 0x42, 0x0F] => :ascii, [0x1B, 0x24, 0x42, 0x0E] => :kana }
  CP5022X_SWITCHES.each { |bytes, g0| states[bytes] = g0 }
  states.each do |prefix, g0|
    pending = (0..0xFF).map { [_1] }
    until pending.empty?
      bytes = pending.shift
      expected = cp5022x_model(g0, bytes)
      expect("#{encoding} after #{prefix} reading #{bytes}", cp5022x_decode(encoding, prefix + bytes), expected)
      pending.concat((0..0xFF).map { bytes + [_1] }) if expected[0] == :incomplete
    end
  end
end

def cp5022x_encode(encoding, bytes)
  bytes.pack("C*").force_encoding("CP51932").encode(encoding).bytes
rescue Encoding::UndefinedConversionError
  :undefined
end

# CP50220's half-width katakana: the cell each is written as alone, and those
# a voiced or semi-voiced mark after them makes one cell with, the next or
# the one after. Every pair of them, and each before another character, is
# checked against what the Rust side builds from these.
def cp50220_kana
  escaped = ->(cells) { [0x1B, 0x24, 0x42, *cells.flat_map { [0x21 + _1 / 94, 0x21 + _1 % 94] }, 0x1B, 0x28, 0x42] }
  cells = (0xA1..0xDF).map do |b|
    bytes = cp5022x_encode("CP50220", [0x8E, b])
    cell = ((bytes[3] || 0) - 0x21) * 94 + (bytes[4] || 0) - 0x21
    expect("CP50220 writing katakana #{b}", bytes, escaped.([cell]))
    cell
  end
  marks = { 0xDE => [1, []], 0xDF => [2, []] }
  (0xA1..0xDF).each do |b|
    marks.each do |mark, (offset, joined)|
      bytes = cp5022x_encode("CP50220", [0x8E, b, 0x8E, mark])
      joined << b if bytes == escaped.([cells[b - 0xA1] + offset])
    end
  end
  voiced, semi_voiced = marks.values.map(&:last)
  (0xA1..0xDF).each do |b|
    cell = cells[b - 0xA1]
    (0xA1..0xDF).each do |after|
      joined = (after == 0xDE && voiced.include?(b)) || (after == 0xDF && semi_voiced.include?(b))
      expected = escaped.(joined ? [cell + after - 0xDD] : [cell, cells[after - 0xA1]])
      expect("CP50220 writing katakana #{b} then #{after}", cp5022x_encode("CP50220", [0x8E, b, 0x8E, after]), expected)
    end
    expect("CP50220 writing katakana #{b} then a", cp5022x_encode("CP50220", [0x8E, b, 0x61]), escaped.([cell])[0...-3] + [0x1B, 0x28, 0x42, 0x61])
    expect("CP50220 writing katakana #{b} then 8EE0", cp5022x_encode("CP50220", [0x8E, b, 0x8E, 0xE0]),
           escaped.([cell])[0...-3] + [0x1B, 0x28, 0x49, 0x60, 0x1B, 0x28, 0x42])
  end
  [cells, voiced, semi_voiced]
end

# Every CP51932 character alone, from ASCII: ASCII but SO, SI and ESC, which
# have no spelling, and the rest as their seven bits after the escape for
# their set — CP50220's katakana aside.
def check_cp5022x_encoder(encoding)
  (0..0x7F).each do |b|
    expect("#{encoding} writing #{b}", cp5022x_encode(encoding, [b]), [0x0E, 0x0F, 0x1B].include?(b) ? :undefined : [b])
  end
  (0xA1..0xFE).each do |b|
    next if encoding == "CP50220" && b <= 0xDF
    expect("#{encoding} writing 8E #{b}", cp5022x_encode(encoding, [0x8E, b]), [0x1B, 0x28, 0x49, b & 0x7F, 0x1B, 0x28, 0x42])
  end
  CELLS.times do |cell|
    a, b = euc(cell)
    expect("#{encoding} writing cell #{cell}", cp5022x_encode(encoding, [a, b]), [0x1B, 0x24, 0x42, a & 0x7F, b & 0x7F, 0x1B, 0x28, 0x42])
  end
end

def planes
  jis0208 = Array.new(CELLS) { |cell| code("EUC-JP", euc(cell)) }
  jis0212 = Array.new(CELLS) { |cell| code("EUC-JP", euc(cell, supplementary: true)) }
  windows = Array.new(120 * 94) { |cell| code("Windows-31J", sjis(cell)) }
  CELLS.times { |cell| expect("Shift_JIS cell #{cell}", code("Shift_JIS", sjis(cell)), jis0208[cell]) }
  (CELLS...120 * 94).each { |cell| expect("Shift_JIS cell #{cell}", code("Shift_JIS", sjis(cell)), nil) }
  cp51932 = windows.first(CELLS)
  CELLS.times { |cell| expect("CP51932 cell #{cell}", code("CP51932", euc(cell)), cp51932[cell]) }
  { "EUC-JP" => jis0208 + jis0212, "Shift_JIS" => jis0208, "Windows-31J" => windows, "CP51932" => cp51932, jis0208:, jis0212: }
end

# How CRuby writes each character, against what reading every cell back
# implies: the cells it does not choose when several read as one character,
# and the characters it writes as a cell that reads as another.
def writing(encoding, cells)
  written = {}
  (0x80..0x10FFFF).each do |point|
    next if point.between?(0xD800, 0xDFFF) || point.between?(0xFF61, 0xFF9F)
    bytes = begin
      point.chr("UTF-8").encode(encoding).bytes
    rescue Encoding::UndefinedConversionError
      next
    end
    fail!("#{encoding} writes U+#{point.to_s(16)} past the BMP") if point > 0xFFFF
    written[point] = cell_of(encoding, bytes)
  end
  one_way = written.select { |point, cell| cells[cell] != point }.sort
  unchosen = cells.each_index.select { |cell| cells[cell] && written[cells[cell]] != cell }
  # What the Rust side builds from these, which must be what CRuby does.
  built = {}
  cells.each_with_index { |point, cell| built[point] = cell if point && !unchosen.include?(cell) }
  one_way.each { |point, cell| built[point] = cell }
  expect("#{encoding} writing", built.sort, written.sort)
  [built.size, unchosen, one_way]
end

def row_text(points)
  points.map do |point|
    next "\\0" if point.nil?
    char = point.chr("UTF-8")
    char.match?(/[^\p{L}\p{N}\p{P}\p{S}]|["\\]/) ? format("\\u{%X}", point) : char
  end.join
end

def plane_text(name, doc, points)
  rows = points.each_slice(94).map { |row| row_text(row) }
  <<~RUST
    #{doc.lines.map { "/// #{_1}" }.join.chomp}
    pub const #{name}: &str = "\\
    #{rows.join("\\\n")}";
  RUST
end

def list(values, per_line)
  return "&[]" if values.empty?
  "&[\n#{values.each_slice(per_line).map { |line| "    #{line.join(", ")}," }.join("\n")}\n]"
end

def render
  ENCODINGS.each do |encoding|
    check_reader(encoding)
    check_arithmetic(encoding)
  end
  check_direct
  CP5022X.each do |encoding|
    check_cp5022x_decoder(encoding)
    check_cp5022x_encoder(encoding)
  end
  kana, voiced, semi_voiced = cp50220_kana
  planes = self.planes
  writes = ENCODINGS.to_h { |encoding| [encoding, writing(encoding, planes[encoding])] }
  sections = [
    plane_text("JIS0208", "JIS X 0208 as EUC-JP and Shift_JIS read it: a row per line, `\\0` for\na cell with no character.", planes[:jis0208]),
    plane_text("JIS0212", "JIS X 0212, EUC-JP's cells after 0x8F.", planes[:jis0212]),
    plane_text("WINDOWS_31J", "Windows-31J's 120 rows: JIS X 0208 with Microsoft's choices, the NEC\nand IBM extensions, and the user-defined rows as private use.", planes["Windows-31J"])
  ]
  writes.each do |encoding, (count, unchosen, one_way)|
    name = encoding.upcase.tr("-", "_")
    sections << <<~RUST
      /// How many characters #{encoding} writes as a cell.
      pub const #{name}_WRITTEN: usize = #{count};

      /// #{encoding}'s cells that read as a character CRuby writes as another cell.
      #[rustfmt::skip]
      pub const #{name}_UNCHOSEN: &[u16] = #{list(unchosen, 12)};

      /// Characters #{encoding} writes as a cell that reads as another, and that cell.
      #[rustfmt::skip]
      pub const #{name}_ONE_WAY: &[(u16, u16)] = #{list(one_way.map { |point, cell| format("(0x%04X, %d)", point, cell) }, 6)};
    RUST
  end
  sections << <<~RUST
    /// The JIS X 0208 cell CP50220 writes for each half-width katakana, 0xA1 to
    /// 0xDF, as it writes them full-width (#419).
    #[rustfmt::skip]
    pub const CP50220_KANA: &[u16] = #{list(kana, 12)};

    /// The katakana CP50220 writes with a voiced mark after it as the next cell.
    #[rustfmt::skip]
    pub const CP50220_VOICED: &[u8] = #{list(voiced.map { format("0x%02X", _1) }, 12)};

    /// The katakana CP50220 writes with a semi-voiced mark after it as the cell
    /// after the next.
    #[rustfmt::skip]
    pub const CP50220_SEMI_VOICED: &[u8] = #{list(semi_voiced.map { format("0x%02X", _1) }, 12)};
  RUST
  <<~RUST
    //! How CRuby maps EUC-JP, Shift_JIS, Windows-31J and CP51932 to Unicode
    //! (#378, #419), and how CP50220 writes the half-width katakana.
    //! Generated by `scripts/jis-table.rb --generate` from ruby #{RUBY_VERSION}; do not edit.
    //!
    //! A plane is its characters in cell order, as text: `jis.rs` reads it into
    //! an array of codepoints at compile time, and inverts it there for writing.

    #{sections.join("\n")}
  RUST
    .sub(/\n+\z/, "\n")
end

case ARGV.first
when "--generate"
  File.write(TABLE, render)
  puts "wrote #{TABLE}"
when "--check"
  if File.read(TABLE).sub(/from ruby [\d.]+/, "") == render.sub(/from ruby [\d.]+/, "")
    puts "#{TABLE} matches ruby #{RUBY_VERSION}"
  else
    warn "#{TABLE} disagrees with ruby #{RUBY_VERSION}: run --generate and review the diff"
    exit 1
  end
else
  warn "usage: #{$PROGRAM_NAME} --check | --generate"
  exit 2
end
