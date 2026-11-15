#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/src/codepage_table.rs` (#41): how CRuby
# maps its single-byte code pages to Unicode and back — ISO-8859-2 to -16,
# KOI8-R and -U, the Windows and IBM code pages, the Mac ones, TIS-620 —
# which CRuby's `enc/trans/single_byte.trans` builds its converters from. As
# with EUC-KR (`scripts/euckr-table.rb`, #725), the mapping is measured
# through `String#encode` on a real Ruby rather than copied from a standard:
# what CRuby's compiled tables say is the answer.
#
#   ruby scripts/codepage-table.rb --check      # CI: agree, or fail
#   ruby scripts/codepage-table.rb --generate   # rewrite the table
#
# A code page is every encoding CRuby converts to and from UTF-8, in one
# converter each way, whose every byte is a character on its own, but for
# BINARY, US-ASCII and ISO-8859-1, which `transcode.rs` does by arithmetic.
# The table is each page's upper half, the bytes 0x80 to 0xFF, as text; the
# lower half is ASCII, which is checked here, not tabled. Besides the half,
# the table records what reading does not determine: the characters CRuby
# writes as more than one byte, which Windows-1255 does for Hebrew's
# presentation forms. What the Rust and Ruby sides assume beyond the table —
# that every page writes each character it reads as the byte it read it
# from, that nothing is written past the BMP, that a page's converters are
# named by its name upcased, and which pages there are — is checked against
# CRuby here too, so a Ruby that changes any of it fails CI.

TABLE = File.expand_path("../crates/spinel-vm/src/codepage_table.rs", __dir__)
TRANSCODE_RB = File.expand_path("../core/transcode.rb", __dir__)
ARITHMETIC = [Encoding::BINARY, Encoding::US_ASCII, Encoding::ISO_8859_1].freeze

def fail!(message)
  warn "codepage-table: #{message}"
  exit 1
end

def expect(what, actual, expected)
  fail!("#{what}: CRuby says #{actual.inspect}, the Rust side assumes #{expected.inspect}") unless actual == expected
end

def path(from, to)
  Encoding::Converter.search_convpath(from, to)
rescue Encoding::ConverterNotFoundError
  nil
end

def pages
  Encoding.list.select do |encoding|
    next false if encoding.dummy? || ARITHMETIC.include?(encoding) || !encoding.ascii_compatible?
    next false unless (0..0xFF).all? { |b| b.chr.force_encoding(encoding).valid_encoding? }
    path(encoding, "UTF-8") == [[encoding, Encoding::UTF_8]] && path("UTF-8", encoding) == [[Encoding::UTF_8, encoding]]
  end
end

# The character byte `b` reads as, nil when CRuby has none for it.
def read(encoding, b)
  b.chr.force_encoding(encoding).encode("UTF-8")
rescue Encoding::UndefinedConversionError
  nil
end

def name_of_converter(encoding)
  # A byte with no character names the converter from the page; a
  # character no page has names the one to it.
  undefined = (0x80..0xFF).find { |b| read(encoding, b).nil? }
  from =
    begin
      undefined&.chr&.force_encoding(encoding)&.encode("UTF-8")
    rescue Encoding::UndefinedConversionError => e
      e.source_encoding_name
    end
  to =
    begin
      "\u{10000}".encode(encoding)
    rescue Encoding::UndefinedConversionError => e
      e.destination_encoding_name
    end
  inspected = Encoding::Converter.new(encoding, "UTF-8").inspect[/: (\S+) to/, 1]
  expect("#{encoding}'s converter name", [from || to, to, inspected].uniq, [encoding.name.upcase])
end

def check_ascii(encoding)
  (0..0x7F).each do |b|
    expect("#{encoding} reading ASCII #{b}", read(encoding, b), b.chr)
    expect("#{encoding} writing ASCII #{b}", b.chr.encode(encoding).bytes, [b])
  end
end

# How CRuby writes each character past ASCII, against what reading the upper
# half implies: every character a byte reads as is written as that byte, and
# the rest are written as several bytes or not at all.
def writing(encoding, upper)
  read_back = {}
  upper.each_with_index { |char, i| read_back[char.ord] = 0x80 + i if char }
  expect("#{encoding}: two bytes reading as one character", read_back.size, upper.compact.size)
  wide = []
  chars = (0x80..0xFFFF).reject { _1.between?(0xD800, 0xDFFF) }
  chars.each do |point|
    bytes = point.chr("UTF-8").encode(encoding, undef: :replace, replace: "").bytes
    if read_back.key?(point)
      expect("#{encoding} writing U+#{format("%04X", point)}", bytes, [read_back[point]])
    elsif !bytes.empty?
      fail!("#{encoding} writes U+#{format("%04X", point)} as the one byte #{bytes}") if bytes.size == 1
      wide << [point, bytes]
    end
  end
  beyond = (0x10000..0x10FFFF).map { _1.chr("UTF-8") }.join.encode(encoding, undef: :replace, replace: "")
  expect("#{encoding} writing past the BMP", beyond, "")
  wide
end

def text(chars)
  chars.map do |char|
    next "\\0" if char.nil?
    char.match?(/[^\p{L}\p{N}\p{P}\p{S}]|["\\]/) ? format("\\u{%X}", char.ord) : char
  end.join
end

def bytes_literal(bytes) = "&[#{bytes.map { format("0x%02X", _1) }.join(", ")}]"

def page_rust(encoding)
  check_ascii(encoding)
  name_of_converter(encoding)
  upper = (0x80..0xFF).map { |b| read(encoding, b) }
  fail!("#{encoding}: a byte reads as more than one character") if upper.compact.any? { _1.length != 1 }
  wide = writing(encoding, upper)
  wide_rust =
    if wide.empty?
      "&[]"
    else
      "&[\n#{wide.map { |point, bytes| "            (0x#{format("%04X", point)}, #{bytes_literal(bytes)}),\n" }.join}        ]"
    end
  [
    "    // #{encoding.name}",
    "    Page {",
    "        index: #{Encoding.list.index(encoding)},",
    "        upper: \"#{text(upper)}\",",
    "        wide: #{wide_rust},",
    "    },"
  ].join("\n")
end

def render
  list = pages
  <<~RUST
    //! How CRuby maps its single-byte code pages to Unicode (#41).
    //! Generated by `scripts/codepage-table.rb --generate` from ruby #{RUBY_VERSION}; do not edit.
    //!
    //! Each page is its encoding's index and its upper half, the bytes 0x80
    //! to 0xFF, as text: `codepage.rs` reads it into codepoints at compile
    //! time, and inverts it there for writing. The lower half is ASCII.

    use crate::codepage::Page;

    /// Every code page, in CRuby's index order: `\\0` for a byte with no
    /// character, and the characters written as more than one byte.
    #[rustfmt::skip]
    pub const PAGES: &[Page] = &[
    #{list.map { |encoding| page_rust(encoding) }.join("\n")}
    ];
  RUST
end

# `core/transcode.rb` keeps the same list by index, as `CODE_PAGES`.
def ruby_list = "  CODE_PAGES = #{pages.map { Encoding.list.index(_1) }.inspect}.freeze"

def check_ruby_list
  line = File.readlines(TRANSCODE_RB, chomp: true).find { _1.start_with?("  CODE_PAGES = ") }
  return if line == ruby_list
  warn "#{TRANSCODE_RB}'s CODE_PAGES disagrees with ruby #{RUBY_VERSION}: run --generate and review the diff"
  exit 1
end

case ARGV.first
when "--generate"
  File.write(TABLE, render)
  source = File.read(TRANSCODE_RB)
  File.write(TRANSCODE_RB, source.sub(/^  CODE_PAGES = .*$/, ruby_list)) if source.match?(/^  CODE_PAGES = /)
  check_ruby_list
  puts "wrote #{TABLE}"
when "--check"
  if File.read(TABLE).sub(/from ruby [\d.]+/, "") == render.sub(/from ruby [\d.]+/, "")
    check_ruby_list
    puts "#{TABLE} matches ruby #{RUBY_VERSION}"
  else
    warn "#{TABLE} disagrees with ruby #{RUBY_VERSION}: run --generate and review the diff"
    exit 1
  end
else
  warn "usage: #{$PROGRAM_NAME} --check | --generate"
  exit 2
end
