#!/usr/bin/env ruby
# frozen_string_literal: true

# The anti-false-pass check.
#
# A spec runner's pass count is only worth anything if a pass means what it says.
# This script takes every example `spec-harness` reports as `passed`, slices the
# `it` block back out of the spec file, and runs it on a real Ruby with a
# four-line mspec shim. If Ruby raises where Spinel did not, or an expectation
# does not hold, then Spinel passed an example it had no right to — which is
# strictly worse than reporting it blocked.
#
#   scripts/verify-passes.rb [dir-or-file ...]     # default: language/
#
# It is the counterpart to `eval-oracle.rb`. That one checks Spinel against Ruby
# on snippets a human chose; this one checks it on every example it actually
# claims, so a construct nobody thought to put in the table is still covered.

require "open3"

ROOT = File.expand_path("..", __dir__)
HARNESS = File.join(ROOT, "target", "release", "spec-harness")

abort "build the harness first: cargo build --release -p spec-harness" unless File.executable?(HARNESS)

paths = ARGV.empty? ? [File.join(ROOT, "spec", "ruby", "language")] : ARGV
listing, status = Open3.capture2(HARNESS, "--list", *paths)
abort "spec-harness --list failed" unless status.success?

class SpecFailure < StandardError; end

# `x.should == y` is `(x.should) == y`, so `should` returns something whose `==`
# is the assertion. Exactly the shape the harness recognises, which is the point:
# if the two disagree about what an example asserts, this catches it.
class ShouldProxy
  def initialize(value, negated)
    @value = value
    @negated = negated
  end

  def ==(other)
    held = (@value == other)
    held = !held if @negated
    unless held
      raise SpecFailure, "#{@value.inspect} should#{@negated ? " not" : ""} equal #{other.inspect}"
    end

    true
  end
end

class Object
  def should = ShouldProxy.new(self, false)
  def should_not = ShouldProxy.new(self, true)
end

# `it "..." do ... end` at the top level of the eval'd slice: just run the block.
def it(_description = nil, &block) = block.call

# The magic comments in a file's leading comment block, as lines to re-emit.
#
# Only the ones that change the meaning of code rather than the parse: an
# `encoding` comment would also matter, but the corpus is UTF-8 throughout and
# `eval` of a UTF-8 String is already UTF-8.
MAGIC = /\A#\s*frozen_string_literal:\s*(?:true|false)\s*\z/
def magic_comments(source)
  source.force_encoding("UTF-8").lines.take(5).grep(MAGIC).join
end

sources = Hash.new { |cache, path| cache[path] = File.binread(path) }
checked = 0
wrong = []

listing.each_line do |line|
  path, outcome, span, description = line.chomp.split("\t", 4)
  next unless outcome == "passed"

  # Comma-separated: the `before` bodies the harness prepended, outermost
  # first, then the example's own. Concatenating them rebuilds exactly what
  # Spinel ran — an example whose helper method is defined in a hook is not the
  # same program without it, and eval'ing only the `it` block would report a
  # false pass that is really this script disagreeing with the harness.
  source = sources[File.join(ROOT, path)]
  text = span.split(",").map { |range|
    first, last = range.split("-").map(&:to_i)
    source.byteslice(first, last - first).force_encoding("UTF-8")
  }.join("\n")
  # A magic comment governs its whole file, and slicing an example out of one
  # leaves it behind — which changes the answer rather than the syntax:
  # `(+s).equal?(s)` is true for a mutable literal and false for a frozen one.
  # Ruby honours these at the top of an eval'd string, so carry them over.
  text = "#{magic_comments(source)}#{text}" unless magic_comments(source).empty?
  checked += 1

  begin
    # A fresh binding per example so one cannot leak a local into the next.
    eval(text, TOPLEVEL_BINDING.dup, path, 1) # rubocop:disable Security/Eval
  rescue SpecFailure => e
    wrong << "#{path}: #{description}\n  spinel passed it; ruby says #{e.message}"
  rescue StandardError, SyntaxError => e
    wrong << "#{path}: #{description}\n  spinel passed it; ruby raised #{e.class}: #{e.message}"
  end
end

if checked.zero?
  abort "no passing examples to verify — that is itself a regression"
elsif wrong.empty?
  puts "#{checked} passing example(s) re-run on #{RUBY_ENGINE} #{RUBY_VERSION}: all agree"
else
  wrong.each { |w| warn w }
  abort "#{wrong.length} example(s) passed in Spinel but not in Ruby"
end
