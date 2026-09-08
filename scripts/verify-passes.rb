#!/usr/bin/env ruby
# frozen_string_literal: true

require "set"

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
# The harness exits non-zero when any example failed, and this script's job is
# to check the ones that *passed* — a run with failures in it still has a
# listing worth reading, and refusing it would mean the anti-false-pass check
# switches itself off exactly when the suite is most in flux. An empty listing
# is still fatal, and the `checked.zero?` guard at the bottom catches it.
listing, _status = Open3.capture2(HARNESS, "--list", *paths)
abort "spec-harness --list produced nothing" if listing.strip.empty?

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
      ::Kernel.raise SpecFailure,
                     "#{@value.inspect} should#{@negated ? " not" : ""} equal #{other.inspect}"
    end

    true
  end

  # `-> { ... }.should.raise(ArgumentError)` — mspec's spelling, and the matcher
  # `spec/harness` learned in #12. The two must agree about what an example
  # asserts: a harness that grows a matcher this script does not have is a
  # harness whose new passes nobody checks, which is the whole point of the file.
  #
  # `::Kernel.raise` throughout, because `raise` is this method's own name here.
  def raise(*expected)
    klass = expected.first
    raised = nil
    begin
      @value.call
    rescue Exception => e # rubocop:disable Lint/RescueException
      raised = e
    end

    if @negated
      unless raised.nil?
        ::Kernel.raise SpecFailure, "should not have raised, but raised #{raised.class}"
      end
      return true
    end

    if raised.nil?
      ::Kernel.raise SpecFailure, "should raise #{klass || "an exception"}, raised nothing"
    end
    if klass && !raised.is_a?(klass)
      ::Kernel.raise SpecFailure, "should raise #{klass}, raised #{raised.class}"
    end

    true
  end
end

# mspec's recorder, which `spec/harness` now provides too. This is the real one:
# if an example depends on `<<` mutating the array `recorded` already handed out
# — which the harness's copy-on-append version cannot do — it fails here, which
# is exactly the check that makes that shortcut safe to have taken.
class ScratchPad
  class << self
    def clear = @record = nil
    def record(object) = @record = object
    def recorded = @record
    def <<(object) = @record << object
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

# `spec/harness` resolves `require_relative` against the requiring file's own
# directory and evaluates what it finds before the example (#183). This script
# has to do the same, or every example that depends on a fixture constant is
# re-run here without it, Ruby raises `NameError`, and the report is a wall of
# false passes that are really this script disagreeing with the harness.
#
# `require` of a library name is ignored on both sides: the harness has no
# `$LOAD_PATH` (#39), and `spec_helper.rb` reaching for mspec raises `LoadError`
# here. Both mean the same thing — the example stays blocked in Spinel — so a
# fixture that will not load is skipped rather than fatal.
#
# **Depth first, and transitively**, because `loader.rs` is: it walks what a
# fixture itself requires before running the fixture's body, which is the order
# Ruby gives it. Scanning only the spec file's own `require_relative` lines is
# one level shallower than the harness, and the difference is not theoretical —
# `core/range/case_compare_spec.rb` requires `shared/cover`, which requires
# `fixtures/classes`, which is where `RangeSpecs` lives. Spinel loaded it and
# passed the example; this script did not and reported a false pass against a
# `NameError` of its own making.
REQUIRE_RELATIVE = /^\s*require_relative\s+(["'])(.+?)\1/
def load_fixtures(path, seen = Set.new)
  seen << File.expand_path(path)
  File.binread(path).force_encoding("UTF-8").scan(REQUIRE_RELATIVE) do |_quote, target|
    fixture = File.expand_path(target, File.dirname(path))
    fixture += ".rb" unless fixture.end_with?(".rb")
    next unless File.file?(fixture)
    # A diamond must not define anything twice and a cycle must not recurse
    # for ever. One set covers both, exactly as `loader.rs`'s does.
    next unless seen.add?(fixture)

    # Depth first: what this fixture requires has to be defined before its own
    # body runs.
    load_fixtures(fixture, seen)

    begin
      # A fixture that gives up writes to stderr on the way out — `spec_helper`
      # prints its "add -Ipath/to/mspec/lib" line — and one copy per example
      # would bury the report. The outcome is already handled below.
      saved = $stderr
      $stderr = File.new(File::NULL, "w")
      begin
        require fixture
      ensure
        $stderr.close
        $stderr = saved
      end
    rescue LoadError, StandardError, SyntaxError, SystemExit
      # Same outcome as the harness failing to compile it. `SystemExit` is in
      # the list because `spec_helper.rb` calls `abort` when mspec is not on the
      # load path, and a fixture that will not load must not take this script
      # down with it.
      nil
    end
  end
end

loaded_fixtures = {}
sources = Hash.new { |cache, path| cache[path] = File.binread(path) }
# Run one example in a child process, and answer what went wrong, or `nil`.
#
# The child writes its verdict down a pipe rather than using an exit status,
# because the message is what makes a disagreement actionable.
def run_isolated(text, path)
  read, write = IO.pipe
  pid = fork do
    read.close
    verdict =
      begin
        eval(text, TOPLEVEL_BINDING.dup, path, 1) # rubocop:disable Security/Eval
        nil
      rescue SpecFailure => e
        "says #{e.message}"
      rescue StandardError, SyntaxError, NotImplementedError => e
        "raised #{e.class}: #{e.message}"
      end
    write.write(verdict.to_s)
    write.close
    # `exit!`, not `exit`: an example that installed an `at_exit` must not run
    # it here, and the parent is the one that reports.
    exit!(0)
  end
  write.close
  verdict = read.read
  read.close
  Process.wait(pid)
  # A child that died without writing — a segfault, an `exit!` inside the
  # example — is itself a disagreement worth seeing.
  return "left the process in #{$?.inspect}" unless $?.success?

  verdict.empty? ? nil : verdict
end

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
  full = File.join(ROOT, path)
  loaded_fixtures[full] ||= (load_fixtures(full) || true)
  source = sources[full]
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

  # A process per example, not just a fresh binding.
  #
  # A binding isolates locals and nothing else, and ruby/spec is full of
  # examples that write to the object space: `case_spec.rb` has
  # `case (def foo; 'foo'; end; 'f')`, whose top-level `def` lands on `Object`
  # and stays there. A later `Class.new(sup) { def foo; super; end }` in
  # `super_spec.rb` then finds it, and the example that asserts `super` raises
  # is reported as Spinel passing something Ruby does not — when what actually
  # happened is that this script let one example rewrite another's world.
  #
  # Spinel gives every example a fresh `Heap`, so the leak is this script's
  # alone. A fork per example is the same isolation, and the cost is a few
  # seconds across the corpus.
  if (message = run_isolated(text, path))
    wrong << "#{path}: #{description}\n  spinel passed it; ruby #{message}"
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
