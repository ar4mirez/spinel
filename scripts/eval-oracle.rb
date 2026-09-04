#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/tests/eval.txt`.
#
# The table is a list of Ruby snippets and the value CRuby produces for each.
# `crates/spinel-vm/tests/eval.rs` runs the same snippets through Spinel's
# compiler and interpreter and holds them to the same answers, which makes the
# expectations a measurement rather than one engineer's belief about what Ruby
# does — and Ruby's answers for floored division, modulo sign, truthiness of 0,
# and what `&&` returns are exactly where a belief goes wrong.
#
#   ruby scripts/eval-oracle.rb --check      # CI: agree, or fail
#   ruby scripts/eval-oracle.rb --generate   # rewrite the `#=>` results
#
# Format: `<snippet>  #=> <result>`, one per line, `#` comments and blank lines
# ignored. Snippets must terminate and must not raise.

TABLE = File.expand_path("../crates/spinel-vm/tests/eval.txt", __dir__)
SEPARATOR = "  #=> "

mode = ARGV[0] || "--check"
abort "usage: #{$0} [--check|--generate]" unless %w[--check --generate].include?(mode)

lines = File.readlines(TABLE)
failures = 0

lines.each_with_index do |line, index|
  next if line.strip.empty? || line.lstrip.start_with?("#")

  source, want = line.chomp.split(SEPARATOR, 2)
  if want.nil?
    warn "#{TABLE}:#{index + 1}: no `#{SEPARATOR.strip}` on a non-comment line"
    failures += 1
    next
  end

  # A fresh binding per snippet: the table's cases are independent, and a local
  # leaking from one into the next would make a passing case depend on its
  # neighbours.
  got =
    begin
      eval(source, TOPLEVEL_BINDING.dup).inspect # rubocop:disable Security/Eval
    rescue StandardError, SyntaxError => e
      warn "#{TABLE}:#{index + 1}: #{source.inspect} raised #{e.class}: #{e.message}"
      failures += 1
      next
    end

  next if got == want

  if mode == "--generate"
    lines[index] = "#{source}#{SEPARATOR}#{got}\n"
  else
    failures += 1
    warn "#{TABLE}:#{index + 1}: #{source}"
    warn "  table: #{want}"
    warn "  ruby:  #{got}"
  end
end

if mode == "--generate"
  File.write(TABLE, lines.join)
  puts "wrote #{TABLE}"
elsif failures.zero?
  puts "#{TABLE} matches #{RUBY_ENGINE} #{RUBY_VERSION}"
else
  abort "#{failures} case(s) disagree with #{RUBY_ENGINE} #{RUBY_VERSION}"
end
