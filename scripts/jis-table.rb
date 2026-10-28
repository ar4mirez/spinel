#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/src/jis_table.rs` (#378): how CRuby maps
# EUC-JP, Shift_JIS and Windows-31J to Unicode and back. The mapping is a table,
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
# two-byte Shift_JIS code, its vendor rows included. ASCII and the half-width
# katakana are arithmetic and are checked here, not tabled.
#
# Besides the planes, the table records what decoding does not determine:
# which cell CRuby writes for a character more than one cell decodes to, and
# the characters it writes as a cell that reads back as another. Everything
# else the Rust side does by hand — which bytes lead, which follow, what an
# invalid sequence reports, the direct EUC-JP and Shift_JIS converter — is
# checked against CRuby here too, so a Ruby that changes any of it fails CI.

TABLE = File.expand_path("../crates/spinel-vm/src/jis_table.rs", __dir__)
CELLS = 94 * 94
ENCODINGS = %w[EUC-JP Shift_JIS Windows-31J].freeze

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
  if encoding == "EUC-JP"
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
  if encoding == "EUC-JP"
    return :char if first < 0x80
    return [:invalid, 1, 0] unless first == 0x8E || first == 0x8F || trail.(first)
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
    bytes = encoding == "EUC-JP" ? [0x8E, b] : [b]
    expect("#{encoding} katakana #{b}", code(encoding, bytes), 0xFF61 + b - 0xA1)
    expect("#{encoding} writing katakana #{b}", (0xFF61 + b - 0xA1).chr("UTF-8").encode(encoding).bytes, bytes)
  end
  return unless encoding == "EUC-JP"
  (0xE0..0xFE).each { |b| expect("EUC-JP 8E #{b}", decode("EUC-JP", [0x8E, b]), [:undefined]) }
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

def planes
  jis0208 = Array.new(CELLS) { |cell| code("EUC-JP", euc(cell)) }
  jis0212 = Array.new(CELLS) { |cell| code("EUC-JP", euc(cell, supplementary: true)) }
  windows = Array.new(120 * 94) { |cell| code("Windows-31J", sjis(cell)) }
  CELLS.times { |cell| expect("Shift_JIS cell #{cell}", code("Shift_JIS", sjis(cell)), jis0208[cell]) }
  (CELLS...120 * 94).each { |cell| expect("Shift_JIS cell #{cell}", code("Shift_JIS", sjis(cell)), nil) }
  { "EUC-JP" => jis0208 + jis0212, "Shift_JIS" => jis0208, "Windows-31J" => windows, jis0208:, jis0212: }
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
  <<~RUST
    //! How CRuby maps EUC-JP, Shift_JIS and Windows-31J to Unicode (#378).
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
