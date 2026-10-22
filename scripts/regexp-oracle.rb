#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-regex/tests/oracle.txt`.
#
# Ruby's regex dialect is Onigmo's, and the interesting parts of it are exactly
# the parts that cannot be read off a reference page and retyped: whether `\w`
# is ASCII, whether `[[:alpha:]]` is not, where a capture lands after an empty
# iteration, what a backreference to an unmatched group does. Every one of those
# is measured here, from a real Ruby, and CI re-checks it.
#
#   ruby scripts/regexp-oracle.rb --check       # CI: agree, or fail
#   ruby scripts/regexp-oracle.rb --generate    # rewrite the table
#   ruby scripts/regexp-oracle.rb --survey      # the engine-choice measurement
#
# The pattern corpus is every regexp literal that appears in ruby/spec's
# `language/regexp/`, which is the set the slice is judged on, plus a handful of
# hand-added patterns for behaviour the corpus exercises only indirectly. Each
# pattern is run against a fixed probe corpus of subjects chosen to separate the
# dialects: newlines for the anchors, non-ASCII for the character classes,
# repeated characters for the quantifiers.
#
# `--survey` is the measurement that settled the engine question in
# `docs/engine.md`. It reports how far a candidate backend would get without
# reimplementing the dialect, and is kept so the decision can be re-run rather
# than believed.

require "prism"

ROOT = File.expand_path("..", __dir__)
CORPUS = File.join(ROOT, "spec", "ruby", "language", "regexp")
TABLE = File.join(ROOT, "crates", "spinel-regex", "tests", "oracle.txt")
POSIX_TABLE = File.join(ROOT, "crates", "spinel-regex", "tests", "posix.txt")
ENCODING_TABLE = File.join(ROOT, "crates", "spinel-regex", "tests", "encodings.txt")

# Subjects chosen so that two engines which disagree about the dialect give
# different answers on at least one of them.
INPUTS = [
  "", "foo", "foo\n", "\nfoo", "foo\nbar", "FOO", "a", "ab", "abc", "aaa",
  "foo bar", " foo ", "x\ny", "1", "12", "a1b2", "\t", "é", "あ", "٣",
  "()", "[c]", "aXb", " ", "a b", "foobar", "barfoo", "\r\n", "-", "_"
].freeze

# Behaviour `language/regexp/` leans on but never spells out in one literal.
EXTRA = [
  '(a*)*', '(a*)+', '(?:(a)|b)\1', '(?=(a))', '(?>a+)ab', 'a++ab',
  '[a-z&&[^b]]', '[[:alpha:]]', '[[:digit:]]', '\h', '\H',
  'a{2}?', 'a{2}+', 'a{2,3}?', 'a{2,}?', '(?i:a)b', 'a(?i)b',
  '(?<n>a)\k<n>', 'foo\Z', 'foo\z', '\Afoo', '^bar', 'foo$',
  # #177: the two halves of the capture-restore rule, and two shapes that
  # separate them.
  '^(()|a)*?$', '(?:(a)x|ab)', '((a)x|a)*', '^((())|a)*?$',
  # #205: the two dialect rules the compiler has to enforce, and the shapes
  # that bound them. `\k<0>` names the whole match rather than a group, and one
  # named group anywhere makes every numeric backreference an error — before it
  # as well as after, and for an unnamed group declared earlier. The last four
  # are the ones Ruby still *accepts* with a named group present, so a check
  # written too broadly fails here rather than in a spec months later.
  '\k<0>', '(?<a>a)\1', '(?<a>a)\k<1>', '(a)(?<a>a)\1', '(a)(?<a>a)\k<1>',
  '(a)\1(?<b>b)', '\1(?<a>a)', '(?<a>a)(?:\1)',
  '(?<a>a)[\1]', '(?<a>a)\0', '(?<a>a)\10', '(a)\k<1>',

  # #217: one named group anywhere stops the unnamed ones from being numbered
  # at all, so `(a)` below captures nothing and group 1 is the *named* one.
  # These are the patterns that pin which groups survive and in what order —
  # including a group written before the parser could know a name was coming,
  # and one in the arm of an alternation that did not run.
  '(a)(?<b>b)', '(?<b>b)(a)', '(a)(?<b>b)(c)(?<d>d)', '(?<b>b)(?<a>a)',
  '(?<x>a)(y)?', '(?:a)(?<b>b)', '(?<a>x)|(y)', '((?<b>b))',
  '(?<b>(a))', '(a)(b)'
].freeze

# Patterns whose *meaning* is fine but whose timing is not: Ruby answers a
# pathological backtracker slowly, and this engine refuses. Nothing to compare.
SKIP = ['(a+)+b'].freeze

def literals
  found = []
  Dir[File.join(CORPUS, "**", "*.rb")].sort.each do |file|
    result = Prism.parse_file(file)
    next unless result.success?

    result.value.breadth_first_search do |node|
      found << node.unescaped if node.is_a?(Prism::RegularExpressionNode)
      false
    end
  end
  (found + EXTRA).uniq.reject { |source| SKIP.include?(source) }.sort
end

# One line per pattern: the pattern, then one field per probe subject.
#
# A field is `-` for no match, or `begin,end` in **characters** followed by each
# group's begin,end or `~` when the group took no part. Characters rather than
# bytes because that is what `MatchData#begin` answers, and the point of the
# table is to be the thing Spinel has to reproduce.
def measure(source)
  regexp =
    begin
      Regexp.new(source)
    rescue RegexpError => e
      return "!#{e.message.lines.first.strip}"
    end

  INPUTS.map { |subject| probe(regexp, subject) }.join(" ")
end

def probe(regexp, subject)
  match = regexp.match(subject)
  return "-" if match.nil?

  groups = (1...match.size).map do |i|
    match.begin(i) ? "#{match.begin(i)},#{match.end(i)}" : "~"
  end
  ["#{match.begin(0)},#{match.end(0)}", *groups].join(":")
rescue StandardError => e
  "!#{e.class}"
end

HEADER = <<~TEXT
  # Ruby's regex dialect, measured. Do not edit by hand.
  #
  # Generated by `scripts/regexp-oracle.rb --generate`; CI re-runs it with
  # `--check`, and `crates/spinel-regex/tests/oracle.rs` replays every line
  # through the engine.
  #
  # One pattern per line: the pattern as a Ruby string literal, a tab, then one
  # field per probe subject. A field is `-` for no match, `!message` when the
  # pattern itself is rejected, or `begin,end` in characters for the whole
  # match followed by `:begin,end` per group, with `~` for a group that took
  # no part.
  #
  # The subjects, in order:
TEXT

def render
  lines = literals.map { |source| "#{source.inspect}\t#{measure(source)}" }
  subjects = INPUTS.each_slice(6).map { |slice| "#   #{slice.map(&:inspect).join(' ')}" }
  [HEADER.chomp, *subjects, "", *lines, ""].join("\n")
end

# The measurement that settled the engine question. Reports how a candidate
# backend that is not Onigmo would fare, so the decision in `docs/engine.md`
# can be re-run rather than taken on trust.
def survey
  sources = literals
  valid = sources.reject { |s| measure(s).start_with?("!") }
  puts "patterns in language/regexp/ (plus #{EXTRA.length} hand-added): #{sources.length}"
  puts "  CRuby accepts: #{valid.length}"
  puts "  CRuby rejects: #{sources.length - valid.length}"
  puts
  puts "Dialect features present, which any non-Onigmo backend must supply:"
  {
    "backreference \\1" => /\\\d/,
    "named group (?<n>)" => /\(\?</,
    "lookbehind (?<=|(?<!" => /\(\?<[=!]/,
    "atomic (?>) or possessive" => /\(\?>|[*+?}]\+/,
    "POSIX bracket [[:x:]]" => /\[\[:/,
    "class intersection &&" => /&&/,
    "\\h hex shorthand" => /\\[hH]/,
    "absent operator (?~)" => /\(\?~/,
    "subexpression call \\g<>" => /\\g</,
    "conditional (?(n))" => /\(\?\(/,
    "keep \\K" => /\\K/,
    "\\R or \\X" => /\\[RX]/
  }.each do |name, probe|
    hits = sources.count { |s| s.match?(probe) }
    puts format("  %-28s %3d patterns", name, hits) if hits.positive?
  end
end

# The fourteen POSIX brackets, measured over every codepoint rather than read
# off a reference page.
#
# These are the part of the dialect that is easiest to get *nearly* right: the
# shorthands (`\w`, `\s`) are ASCII in Ruby and the brackets are not, so an
# engine built on Rust's `char` predicates lands close enough that sampling a
# few characters agrees. `[[:blank:]]` is the case that proves sampling is not
# enough — it is Zs plus tab, and `char::is_whitespace` is Zs plus the line
# separators, so the two differ on exactly the characters nobody probes by
# hand. So this table is exhaustive: every scalar value, as ranges.
POSIX = %w[
  alpha digit alnum upper lower space blank print graph cntrl xdigit word
  ascii punct
].freeze

SURROGATES = (0xD800..0xDFFF).freeze

# Every codepoint the bracket matches, as ascending `lo-hi` ranges in hex.
# Surrogates are skipped: they are not scalar values, Ruby cannot build a
# String from one, and Rust's `char` cannot hold one either.
def posix_ranges(name)
  regexp = Regexp.new("[[:#{name}:]]")
  ranges = []
  run_start = nil
  (0..0x10FFFF).each do |cp|
    held =
      if SURROGATES.cover?(cp)
        false
      else
        regexp.match?(cp.chr(Encoding::UTF_8))
      end
    if held
      run_start ||= cp
    elsif run_start
      ranges << [run_start, cp - 1]
      run_start = nil
    end
  end
  ranges << [run_start, 0x10FFFF] if run_start
  ranges
end

def posix_render
  lines = POSIX.map do |name|
    fields = posix_ranges(name).map do |lo, hi|
      lo == hi ? format("%X", lo) : format("%X-%X", lo, hi)
    end
    "#{name}\t#{fields.join(';')}"
  end
  [POSIX_HEADER.chomp, "", *lines, ""].join("\n")
end

POSIX_HEADER = <<~TEXT
  # Ruby's POSIX brackets, measured over every codepoint. Do not edit by hand.
  #
  # Generated by `scripts/regexp-oracle.rb --generate`; CI re-runs it with
  # `--check`, and a unit test in `crates/spinel-regex/src/exec.rs` replays
  # every range through the engine's own class predicate.
  #
  # One bracket per line: the name, a tab, then the codepoints it matches as
  # `;`-separated hex ranges, `LO-HI` or a bare `LO` for a single codepoint.
  # Surrogates are excluded — Ruby cannot build a String from one and Rust's
  # `char` cannot hold one, so neither engine has an answer to compare.
TEXT

# The classes, shorthands, word boundary and case folds of the characters past
# ASCII in the encodings `spinel-regex` reads besides UTF-8 and BINARY (#339).
ENCODING_CLASSES = (POSIX.map { |name| ["[[:#{name}:]]", name] } +
                    [["\\w", "\\w"], ["\\d", "\\d"], ["\\s", "\\s"], ["\\h", "\\h"], ["\\b", "\\b"]]).freeze

# Every character past ASCII, by its code: Onigmo's number for it, the bytes
# read as one big-endian integer.
def encoding_chars(name)
  if name == "ISO-8859-1"
    return (0x80..0xFF).map { |b| [b, b.chr.force_encoding(name)] }
  end
  bytes = []
  euc = (0xA1..0xFE).to_a
  if name == "EUC-JP"
    euc.each { |t| bytes << [0x8E, t] }
    euc.each { |l| euc.each { |t| bytes << [l, t] } }
    euc.each { |m| euc.each { |t| bytes << [0x8F, m, t] } }
  else
    (0xA1..0xDF).each { |b| bytes << [b] }
    trails = (0x40..0x7E).to_a + (0x80..0xFC).to_a
    ((0x81..0x9F).to_a + (0xE0..0xFC).to_a).each { |l| trails.each { |t| bytes << [l, t] } }
  end
  bytes.map { |list| [list.inject(0) { |code, b| (code << 8) | b }, list.pack("C*").force_encoding(name)] }
end

def code_ranges(codes)
  codes.chunk_while { |a, b| b == a + 1 }.map do |run|
    run.size == 1 ? format("%X", run[0]) : format("%X-%X", run[0], run[-1])
  end.join(";")
end

# A class over every character: its codes as ranges, or `all` and `none` for
# the multibyte encodings, whose code space has holes no range list reads well.
def encoding_class(name, pattern, chars)
  regexp = Regexp.new(pattern.encode(name))
  held = chars.select { |_, char| regexp.match?(char) }.map(&:first)
  return code_ranges(held) if name == "ISO-8859-1"
  return "all" if held.size == chars.size
  return "none" if held.empty?
  code_ranges(held)
end

# The pairs `/x/i` matches, as `LO-HI+D` blocks: every code from LO to HI folds
# to itself plus D. The candidates are each character's Unicode other cases,
# transcoded back — Onigmo folds nothing else in these encodings, and trying
# every pair is 10^8 matches.
def encoding_folds(name, chars)
  pairs = chars.flat_map do |code, char|
    # A code no Unicode character maps to has no other case to try.
    unicode = begin
      char.encode(Encoding::UTF_8)
    rescue EncodingError
      next []
    end
    others = [unicode.upcase, unicode.downcase].uniq - [unicode]
    others.filter_map do |other|
      back = begin
        other.encode(name)
      rescue EncodingError
        next
      end
      folded = Regexp.new(Regexp.escape(char), Regexp::IGNORECASE).match?(back)
      [code, back.bytes.inject(0) { |c, b| (c << 8) | b }] if folded && back.bytesize == char.bytesize
    end
  end
  pairs.sort.chunk_while { |(a, x), (b, y)| b == a + 1 && y - b == x - a }.map do |run|
    format("%X-%X%+X", run[0][0], run[-1][0], run[0][1] - run[0][0]).sub("+-", "-")
  end.join(";")
end

def encoding_render
  lines = %w[ISO-8859-1 EUC-JP Windows-31J Shift_JIS].flat_map do |name|
    chars = encoding_chars(name)
    ENCODING_CLASSES.map { |pattern, label| "#{name}\t#{label}\t#{encoding_class(name, pattern, chars)}" } +
      ["#{name}\tfold\t#{encoding_folds(name, chars)}"]
  end
  [ENCODING_HEADER.chomp, "", *lines, ""].join("\n")
end

ENCODING_HEADER = <<~TEXT
  # The characters past ASCII in ISO-8859-1, EUC-JP, Windows-31J and
  # Shift_JIS, as Ruby's regexps class and fold them. Do not edit by hand.
  #
  # Generated by `scripts/regexp-oracle.rb --generate`; CI re-runs it with
  # `--check`, and a unit test in `crates/spinel-regex/src/exec.rs` replays
  # every line through the engine's own predicates.
  #
  # One line per encoding and class: the encoding, a tab, the class, a tab,
  # then the codes it holds — a character's code is its bytes read as one
  # big-endian number — as `;`-separated hex ranges, or `all` or `none` of
  # the multibyte encodings' characters. A `fold` line lists the case pairs
  # as `LO-HI+D`: every code from LO to HI folds to itself plus D.
TEXT

case ARGV[0]
when "--generate"
  File.write(TABLE, render)
  puts "wrote #{TABLE}"
  File.write(POSIX_TABLE, posix_render)
  puts "wrote #{POSIX_TABLE}"
  File.write(ENCODING_TABLE, encoding_render)
  puts "wrote #{ENCODING_TABLE}"
when "--check"
  stale = []
  {TABLE => method(:render), POSIX_TABLE => method(:posix_render),
   ENCODING_TABLE => method(:encoding_render)}.each do |path, build|
    current = File.exist?(path) ? File.read(path) : ""
    stale << path unless current == build.call
  end
  if stale.empty?
    puts "regexp oracle is current"
  else
    warn "regexp oracle is stale: regenerate with scripts/regexp-oracle.rb --generate"
    warn "  ruby #{RUBY_VERSION} disagrees with the checked-in table"
    stale.each { |path| warn "  #{path}" }
    exit 1
  end
when "--survey"
  survey
else
  abort "usage: regexp-oracle.rb --check | --generate | --survey"
end
