#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/tests/coerce.txt`.
#
# Ruby's numeric operators do not fail on an operand they do not recognise: they
# ask it to `coerce` itself, expect `[x, y]` back, and retry the operator on that
# pair. The part that cannot be read off a reference page is where the three
# variants of that rule disagree, and they disagree deliberately:
#
#   * an operand with no `coerce` is a TypeError to `+`, `nil` to `<=>`, and an
#     ArgumentError to `<`;
#   * a `coerce` answering nil is "no opinion" to `<=>` but still a TypeError
#     to `+`;
#   * a `coerce` answering the wrong *shape* is a TypeError to all of them,
#     because that is a broken object rather than a refusal to compare;
#   * the retry runs on the coerced pair, so `4.2 + obj` can end up in
#     `String#+`, and `4.2 < obj` retries `<` rather than `<=>`.
#
# Each line here is CRuby's answer, measured, so the expectations in
# `crates/spinel-vm/tests/coerce.rs` are not one engineer's belief.
#
#   ruby scripts/coerce-oracle.rb --check      # CI: agree, or fail
#   ruby scripts/coerce-oracle.rb --generate   # rewrite the `#=>` results
#
# Format: `<snippet>  #=> <result>`, one per line, `#` comments and blank lines
# ignored. A snippet that raises records `!Class: message`, because most of what
# this slice had to get right is which error Ruby picks. Addresses are
# normalised to `0xXX`: the shape of `#<Class:0x...>` is the rule, the address
# is not.

TABLE = File.expand_path("../crates/spinel-vm/tests/coerce.txt", __dir__)
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

  # A fresh binding per snippet, so a local — or a constant assigned to see
  # where it lands — cannot leak into the next case. Constants still land on
  # Object, which is why every snippet that assigns one uses a unique name.
  got =
    begin
      eval(source, TOPLEVEL_BINDING.dup).inspect # rubocop:disable Security/Eval
    rescue Exception => e # rubocop:disable Lint/RescueException
      "!#{e.class}: #{e.message}"
    end
  got = got.gsub(/0x\h+/, "0xXX")

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
  puts "#{TABLE}: regenerated"
  exit 0
end

abort "#{failures} row(s) disagree with ruby #{RUBY_VERSION}" if failures.positive?
puts "#{TABLE}: agrees with ruby #{RUBY_VERSION}"
