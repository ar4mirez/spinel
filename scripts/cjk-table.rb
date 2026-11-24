#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/src/cjk_table.rs` (#796): how CRuby
# maps the CJK multibyte encodings to Unicode and back — GBK, GB18030,
# GB2312 and GB12345 (`enc/trans/chinese`, `gbk`, `gb18030`), Big5,
# Big5-HKSCS, Big5-UAO, CP950 and CP951 (`enc/trans/big5`), CP949
# (`enc/trans/korean`), and EUC-JIS-2004 and eucJP-ms (`enc/trans/JIS`).
# As with EUC-KR (`scripts/euckr-table.rb`, #725), the mapping is measured
# through `String#encode` on a real Ruby rather than copied from a standard:
# what CRuby's compiled tables say is the answer, vendor quirks included.
#
#   ruby scripts/cjk-table.rb --check      # CI: agree, or fail
#   ruby scripts/cjk-table.rb --generate   # rewrite the table
#
# A character past ASCII is a *cell*, numbered by its bytes. A double-byte
# encoding's cell is its lead byte's place among the leads times the number
# of trail bytes, plus its trail's place among them; an EUC encoding's are
# the 94 after 0x8E, then JIS X 0213's or JIS X 0208's 94 rows of 94, then
# the 94 rows after 0x8F. GB18030's four bytes are a run of codepoints each,
# which the table lists rather than a plane. Besides the planes the table
# records what reading does not determine: the cells CRuby does not write
# for the character they read as, the characters it writes as a cell that
# reads as another, and those it writes as one byte, which CP950 and CP951
# do for accented Latin letters. What `cjk.rs` does by hand — which bytes
# lead, which follow, what an invalid or short sequence reports, how a cell
# is spelled — is checked against CRuby here too, so a Ruby that changes any
# of it fails CI.

TABLE = File.expand_path("../crates/spinel-vm/src/cjk_table.rs", __dir__)

# Each encoding's shape, as `cjk.rs` has it: the leads and trails of its two
# bytes, with GB18030's four and the EUC encodings' 0x8E and 0x8F besides.
BIG5_TRAILS = [0x40..0x7E, 0xA1..0xFE].freeze
GBK_TRAILS = [0x40..0x7E, 0x80..0xFE].freeze
ENCODINGS = {
  "GBK" => { id: "GBK", shape: :double, leads: 0x81..0xFE, trails: GBK_TRAILS },
  "GB18030" => { id: "GB18030", shape: :gb18030, leads: 0x81..0xFE, trails: GBK_TRAILS },
  "GB2312" => { id: "GB2312", shape: :double, leads: 0xA1..0xFE, trails: [0xA1..0xFE] },
  "GB12345" => { id: "GB12345", shape: :double, leads: 0xA1..0xFE, trails: [0xA1..0xFE] },
  "Big5" => { id: "BIG5", shape: :double, leads: 0x81..0xFE, trails: BIG5_TRAILS },
  "Big5-HKSCS" => { id: "BIG5_HKSCS", shape: :double, leads: 0x81..0xFE, trails: BIG5_TRAILS },
  "Big5-UAO" => { id: "BIG5_UAO", shape: :double, leads: 0x81..0xFE, trails: BIG5_TRAILS },
  "CP950" => { id: "CP950", shape: :double, leads: 0x81..0xFE, trails: BIG5_TRAILS },
  "CP951" => { id: "CP951", shape: :double, leads: 0x81..0xFE, trails: BIG5_TRAILS },
  "CP949" => { id: "CP949", shape: :double, leads: 0x81..0xFE, trails: [0x41..0x5A, 0x61..0x7A, 0x81..0xFE] },
  "EUC-JIS-2004" => { id: "EUC_JIS_2004", shape: :euc },
  "eucJP-ms" => { id: "EUCJP_MS", shape: :euc },
}.freeze

EUC_TRAIL = 0xA1..0xFE
GB_DIGIT = 0x30..0x39
GB_FOUR_CELLS = 126 * 10 * 126 * 10
# Where the astral characters a plane holds are numbered from in its `u16`s:
# the surrogates, which no cell reads as.
ASTRAL_BASE = 0xD800
ASTRAL_MAX = 0x800

def fail!(message)
  warn "cjk-table: #{message}"
  exit 1
end

def expect(what, actual, expected)
  fail!("#{what}: CRuby says #{actual.inspect}, the Rust side assumes #{expected.inspect}") unless actual == expected
end

def hex(bytes) = bytes.map { format("%02X", _1) }.join

def decode(encoding, bytes)
  [:char, bytes.pack("C*").force_encoding(encoding).encode("UTF-8")]
rescue Encoding::UndefinedConversionError
  [:undefined]
rescue Encoding::InvalidByteSequenceError => e
  [e.incomplete_input? ? :incomplete : :invalid, e.error_bytes.bytesize, e.readagain_bytes&.bytesize || 0]
end

# The codepoint `bytes` read as, nil when CRuby has no character for them.
def code(encoding, bytes)
  text = bytes.pack("C*").force_encoding(encoding).encode("UTF-8", undef: :replace, replace: "")
  return nil if text.empty?
  fail!("#{encoding} reads #{hex(bytes)} as #{text.codepoints.size} characters") unless text.codepoints.size == 1
  text.ord
end

# What CRuby writes `point` as, nil for nothing.
def written(encoding, point)
  bytes = point.chr("UTF-8").encode(encoding, undef: :replace, replace: "").bytes
  bytes.empty? ? nil : bytes
end

def trail_list(spec) = spec[:trails].flat_map(&:to_a)

def cell_count(spec)
  spec[:shape] == :euc ? 94 + 2 * 94 * 94 : spec[:leads].size * trail_list(spec).size
end

# The bytes of a cell.
def bytes_of(spec, cell)
  if spec[:shape] == :euc
    return [0x8E, 0xA1 + cell] if cell < 94
    cell -= 94
    plane, cell = cell.divmod(94 * 94)
    pair = [0xA1 + cell / 94, 0xA1 + cell % 94]
    plane.zero? ? pair : [0x8F, *pair]
  else
    trails = trail_list(spec)
    [spec[:leads].first + cell / trails.size, trails[cell % trails.size]]
  end
end

# The cell `bytes` spell, nil when they spell none.
def cell_of(spec, bytes)
  if spec[:shape] == :euc
    case bytes
    in [0x8E, b] if EUC_TRAIL.cover?(b) then b - 0xA1
    in [0x8F, a, b] if EUC_TRAIL.cover?(a) && EUC_TRAIL.cover?(b) then 94 + 94 * 94 + (a - 0xA1) * 94 + b - 0xA1
    in [a, b] if EUC_TRAIL.cover?(a) && EUC_TRAIL.cover?(b) then 94 + (a - 0xA1) * 94 + b - 0xA1
    else nil
    end
  else
    trails = trail_list(spec)
    return nil unless bytes.size == 2 && spec[:leads].cover?(bytes[0]) && trails.include?(bytes[1])
    (bytes[0] - spec[:leads].first) * trails.size + trails.index(bytes[1])
  end
end

# GB18030's four bytes as a number from 0, and back.
def gb_index(bytes)
  a, b, c, d = bytes
  (((a - 0x81) * 10 + b - 0x30) * 126 + c - 0x81) * 10 + d - 0x30
end

def gb_bytes(index)
  index, d = index.divmod(10)
  index, c = index.divmod(126)
  a, b = index.divmod(10)
  [0x81 + a, 0x30 + b, 0x81 + c, 0x30 + d]
end

# What `cjk.rs`'s reader assumes about the bytes alone, for every prefix
# CRuby reads: a character, a sequence the input ends inside, or an invalid
# one with how many bytes are the error and how many are read again. A
# character may still be undefined; that is the planes' business.
def model(spec, singles, bytes)
  first, second, third, fourth = bytes
  return :char if first < 0x80 || singles.include?(first)
  if spec[:shape] == :euc
    return [:invalid, 1, 0] unless first == 0x8E || first == 0x8F || EUC_TRAIL.cover?(first)
    return [:incomplete, 1, 0] if second.nil?
    return [:invalid, 1, 1] unless EUC_TRAIL.cover?(second)
    return :char unless first == 0x8F
    return [:incomplete, 2, 0] if third.nil?
    return EUC_TRAIL.cover?(third) ? :char : [:invalid, 2, 1]
  end
  return [:invalid, 1, 0] unless spec[:leads].cover?(first)
  return [:incomplete, 1, 0] if second.nil?
  return :char if trail_list(spec).include?(second)
  return [:invalid, 1, 1] unless spec[:shape] == :gb18030 && GB_DIGIT.cover?(second)
  return [:incomplete, 2, 0] if third.nil?
  return [:invalid, 2, 1] unless (0x81..0xFE).cover?(third)
  return [:incomplete, 3, 0] if fourth.nil?
  GB_DIGIT.cover?(fourth) ? :char : [:invalid, 3, 1]
end

def check_reader(encoding, spec, singles)
  prefixes = (0..0xFF).map { [_1] }
  until prefixes.empty?
    bytes = prefixes.shift
    outcome = decode(encoding, bytes)
    outcome = :char if outcome[0] == :char || outcome[0] == :undefined
    expect("#{encoding} reading #{hex(bytes)}", outcome, model(spec, singles, bytes))
    # GB18030's fourth byte is checked on a sample of what comes before it.
    next unless outcome.is_a?(Array) && outcome[0] == :incomplete
    next if bytes.size == 3 && !(bytes[0] % 23 == 0 || [0x81, 0x84, 0x90, 0xE3, 0xFE].include?(bytes[0]))
    next if bytes.size == 3 && !(bytes[2] % 17 == 0 || [0x81, 0xFE].include?(bytes[2]))
    prefixes.concat((0..0xFF).map { bytes + [_1] })
  end
  expect("#{encoding}: path to UTF-8", Encoding::Converter.search_convpath(encoding, "UTF-8"), [[Encoding.find(encoding), Encoding::UTF_8]])
  expect("#{encoding}: path from UTF-8", Encoding::Converter.search_convpath("UTF-8", encoding), [[Encoding::UTF_8, Encoding.find(encoding)]])
  (0..0x7F).each do |b|
    expect("#{encoding}: ASCII #{b}", code(encoding, [b]), b)
    expect("#{encoding}: writing ASCII #{b}", written(encoding, b), [b])
  end
end

# GB18030's four bytes, read: the runs of numbers that read as consecutive
# codepoints, as [first number, first codepoint, how many]. Every other
# number is undefined.
def gb_runs
  runs = []
  GB_FOUR_CELLS.times do |index|
    point = code("GB18030", gb_bytes(index))
    next if point.nil?
    last = runs.last
    if last && last[0] + last[2] == index && last[1] + last[2] == point
      last[2] += 1
    else
      runs << [index, point, 1]
    end
  end
  expect("GB18030: its four bytes' runs in codepoint order", runs.each_cons(2).all? { |a, b| a[1] + a[2] <= b[1] }, true)
  runs
end

def gb_find(runs, point)
  run = runs.bsearch { |_, first, count| first + count > point }
  run && run[1] <= point ? run[0] + point - run[1] : nil
end

# How CRuby writes each character, against what reading every cell back
# implies: the single bytes it writes, the characters it writes as a cell
# that reads as another, and the cells it does not choose for the character
# they read as. `cjk.rs` looks in that order, then in the cells, then, for
# GB18030, in the runs; that it then agrees with CRuby for every codepoint is
# checked here.
def writing(encoding, spec, singles, cells, runs)
  single = {}
  one_way = {}
  chosen = {}
  cruby = {}
  (0x80..0x10FFFF).each do |point|
    next if point.between?(0xD800, 0xDFFF)
    bytes = cruby[point] = written(encoding, point)
    next if bytes.nil?
    if bytes.size == 1
      fail!("#{encoding} writes U+#{point.to_s(16)} as #{hex(bytes)}") unless bytes[0] < 0x80 || singles.include?(bytes[0])
      single[point] = bytes[0]
    elsif spec[:shape] == :gb18030 && bytes.size == 4
      expect("GB18030 writing U+#{point.to_s(16)}", gb_index(bytes), gb_find(runs, point))
    elsif (cell = cell_of(spec, bytes))
      if cells[cell] == point
        chosen[point] = cell
      else
        one_way[point] = cell
      end
    else
      fail!("#{encoding} writes U+#{point.to_s(16)} as #{hex(bytes)}, not a cell")
    end
  end
  unchosen = cells.each_index.select { |cell| cells[cell] && chosen[cells[cell]] != cell }
  skipped = unchosen.to_h { [_1, true] }
  # What `cjk.rs` writes, built the way it looks, against CRuby.
  two_way = {}
  cells.each_with_index do |point, cell|
    next if point.nil? || skipped[cell]
    fail!("#{encoding}: two cells write U+#{point.to_s(16)}") if two_way.key?(point)
    two_way[point] = cell
  end
  (0x80..0x10FFFF).each do |point|
    next if point.between?(0xD800, 0xDFFF)
    built =
      if single.key?(point) then [single[point]]
      elsif one_way.key?(point) then bytes_of(spec, one_way[point])
      elsif two_way.key?(point) then bytes_of(spec, two_way[point])
      elsif runs && (index = gb_find(runs, point)) then gb_bytes(index)
      end
    expect("#{encoding} writing U+#{point.to_s(16)}", cruby[point], built)
  end
  [two_way.size, unchosen, one_way.sort, single.sort]
end

def row_text(points)
  points.map do |point|
    next "\\0" if point.nil?
    char = point.chr("UTF-8")
    char.match?(/[^\p{L}\p{N}\p{P}\p{S}]|["\\]/) ? format("\\u{%X}", point) : char
  end.join
end

def measure(encoding, spec)
  singles = (0x80..0xFF).select { decode(encoding, [_1])[0] == :char }
  check_reader(encoding, spec, singles)
  single_reads = singles.map { |b| [b, code(encoding, [b])] }
  cells = Array.new(cell_count(spec)) { |cell| code(encoding, bytes_of(spec, cell)) }
  astral = cells.compact.count { _1 > 0xFFFF }
  fail!("#{encoding} reads #{astral} astral characters, past what a plane numbers") if astral > ASTRAL_MAX
  runs = spec[:shape] == :gb18030 ? gb_runs : nil
  count, unchosen, one_way, single = writing(encoding, spec, singles, cells, runs)
  { spec:, single_reads:, cells:, astral:, runs:, count:, unchosen:, one_way:, single: }
end

def list(values, per_line)
  return "&[]" if values.empty?
  "&[\n#{values.each_slice(per_line).map { |line| "    #{line.join(", ")}," }.join("\n")}\n]"
end

# One encoding's module. Everything in it is indented but the plane's rows,
# which a string's line continuation would skip the indentation of anyway.
def render_encoding(encoding, m)
  rows = m[:cells].each_slice(m[:spec][:shape] == :euc ? 94 : trail_list(m[:spec]).size).map { |row| row_text(row) }
  items = [<<~RUST]
    /// #{encoding}'s cells as it reads them: a row per lead, `\\0` for a
    /// cell with no character.
    pub const PLANE: &str = "\\
  RUST
  items << "#{rows.join("\\\n")}\";\n"
  runs = m[:runs]&.map { |i, p, n| format("(%d, 0x%04X, %d)", i, p, n) }
  items << <<~RUST.gsub(/^(?=.)/, "    ")

    /// How many of #{encoding}'s cells read as a character past the BMP.
    pub const ASTRAL: usize = #{m[:astral]};

    /// #{encoding}'s bytes past ASCII that are a character alone, and what
    /// each reads as.
    #[rustfmt::skip]
    pub const SINGLE_READS: &[(u8, u32)] = #{list(m[:single_reads].map { |b, p| format("(0x%02X, 0x%04X)", b, p) }, 6)};

    /// How many characters #{encoding} writes as the cell they read from.
    pub const WRITTEN: usize = #{m[:count]};

    /// #{encoding}'s cells that read as a character CRuby writes otherwise.
    #[rustfmt::skip]
    pub const UNCHOSEN: &[u16] = #{list(m[:unchosen], 12)};

    /// Characters #{encoding} writes as a cell that reads as another, and
    /// that cell.
    #[rustfmt::skip]
    pub const ONE_WAY: &[(u32, u16)] = #{list(m[:one_way].map { |p, c| format("(0x%04X, %d)", p, c) }, 6)};

    /// Characters #{encoding} writes as one byte, and that byte.
    #[rustfmt::skip]
    pub const SINGLE_WRITES: &[(u32, u8)] = #{list(m[:single].map { |p, b| format("(0x%04X, 0x%02X)", p, b) }, 6)};
  RUST
  if runs
    items << <<~RUST.gsub(/^(?=.)/, "    ")

      /// GB18030's four bytes, numbered from 0, that read as consecutive
      /// codepoints: the first number, its codepoint and how many. Every
      /// other number is undefined.
      #[rustfmt::skip]
      pub const FOUR: &[(u32, u32, u32)] = #{list(runs, 4)};
    RUST
  end
  "/// #{encoding} (#796).\npub mod #{m[:spec][:id].downcase} {\n#{items[0].gsub(/^(?=.)/, "    ")}#{items[1..].join}}\n"
end

def render
  body = ENCODINGS.map { |encoding, spec| render_encoding(encoding, measure(encoding, spec)) }
  <<~RUST.gsub(/\n{3,}/, "\n\n")
    //! How CRuby maps the CJK multibyte encodings to Unicode (#796): GBK,
    //! GB18030, GB2312, GB12345, Big5, Big5-HKSCS, Big5-UAO, CP950, CP951,
    //! CP949, EUC-JIS-2004 and eucJP-ms.
    //! Generated by `scripts/cjk-table.rb --generate` from ruby #{RUBY_VERSION}; do not edit.
    //!
    //! A plane is its characters in cell order, as text: `cjk.rs` reads it
    //! into an array at compile time, and inverts it there for writing.

    #{body.join("\n").chomp}
  RUST
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
