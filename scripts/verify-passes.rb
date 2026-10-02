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

require "fileutils"
require "open3"
require "tmpdir"

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
#
# A `BasicObject`, because `x.should.equal?(y)`, `x.should.frozen?` and every
# other predicate mspec accepts on a bare `should` must reach `method_missing`
# rather than find `Object`'s own method — `spec/harness` learned that form in
# #21 and this shim has to agree with it.
class ShouldProxy < BasicObject
  def initialize(value, negated)
    @value = value
    @negated = negated
  end

  # `x.should.name(args)` holds when `x.name(args)` is truthy.
  def method_missing(name, *args, &block)
    held = @value.__send__(name, *args, &block) ? true : false
    held = !held if @negated
    unless held
      ::Kernel.raise ::SpecFailure,
                     "#{@value.inspect} should#{@negated ? " not" : ""} be #{name} #{args.inspect}"
    end

    true
  end

  def respond_to_missing?(_name, _include_all = false) = true

  def equal?(other) = method_missing(:equal?, other)
  def !=(other) = method_missing(:!=, other)
  def !() = method_missing(:!)

  def ==(other)
    held = (@value == other)
    held = !held if @negated
    unless held
      ::Kernel.raise ::SpecFailure,
                     "#{@value.inspect} should#{@negated ? " not" : ""} equal #{other.inspect}"
    end

    true
  end

  # `x.should =~ /re/` — mspec's match matcher, which `spec/harness` learned
  # for #29: it holds when `x =~ re` is truthy.
  def =~(other)
    held = @value =~ other
    held = !held if @negated
    unless held
      ::Kernel.raise ::SpecFailure,
                     "#{@value.inspect} should#{@negated ? " not" : ""} match #{other.inspect}"
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
    rescue ::Exception => e # rubocop:disable Lint/RescueException
      raised = e
    end

    if @negated
      unless raised.nil?
        ::Kernel.raise ::SpecFailure, "should not have raised, but raised #{raised.class}"
      end
      return true
    end

    if raised.nil?
      ::Kernel.raise ::SpecFailure, "should raise #{klass || "an exception"}, raised nothing"
    end
    if klass && !raised.is_a?(klass)
      ::Kernel.raise ::SpecFailure, "should raise #{klass}, raised #{raised.class}"
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
# Both kinds that change what code means: `frozen_string_literal`, and an
# `encoding`/`coding` comment, which sets every plain literal's encoding —
# ruby/spec's files are mostly `# -*- encoding: us-ascii -*-`, and since #19
# Spinel honours that, so a replay without it compares different programs.
MAGIC = /\A#.*(?:\bfrozen_string_literal:\s*(?:true|false)|\b(?:en)?coding[:=]\s*[\w.-]+)/
def magic_comments(source)
  source.dup.force_encoding("UTF-8").scrub.lines.take(5).grep(MAGIC).join
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
  # Scrubbed: some fixtures are in other encodings on purpose
  # (`iso-8859-9-encoding.rb`), and only the ASCII `require_relative` lines
  # matter here.
  File.binread(path).force_encoding("UTF-8").scrub.scan(REQUIRE_RELATIVE) do |_quote, target|
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
#
# The example is `load`ed from a file rather than `eval`ed, because that is how
# mspec runs a spec file and the difference is visible once backtraces are
# (#29): a loaded file's top level is `<top (required)>`, an eval's is `<main>`,
# and `Location#base_label` asserts on it. The file sits at the spec's own
# relative path under a temporary directory, so a backtrace names
# `backtrace_spec.rb` just as the real run's does.
#
# The directory stands in for the spec's own, beside it in the real tree, with
# the spec's neighbours symlinked in: an example that loads a file relative to
# its spec — `require_relative "../../fixtures/..."`, `load` of a path built
# from `__FILE__` — reaches the real file under its real path, as it does in
# Spinel's run and under mspec (#39). A path through a symlinked parent would
# land in `$LOADED_FEATURES` spelled differently from the realpath the spec
# compares it with. One stand-in per spec file, removed at exit.
STAND_INS = {}
at_exit { STAND_INS.each_value { |dir| FileUtils.rm_rf(dir) } }

def stand_in_for(path)
  STAND_INS[path] ||= begin
    real = File.join(ROOT, File.dirname(path))
    dir = Dir.mktmpdir(".verify-passes-", File.dirname(real))
    Dir.each_child(real) do |entry|
      next if entry == File.basename(path)
      File.symlink(File.join(real, entry), File.join(dir, entry))
    end
    dir
  end
end

def run_isolated(text, path)
  dir = stand_in_for(path)
  read, write = IO.pipe
  pid = fork do
    read.close
    verdict =
      begin
        file = File.join(dir, File.basename(path))
        File.binwrite(file, text)
        load file
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
  # A magic comment governs its whole file, and slicing an example out of one
  # leaves it behind — which changes the answer rather than the syntax:
  # `(+s).equal?(s)` is true for a mutable literal and false for a frozen one.
  # Ruby honours these at the top of a file, so carry them over.
  #
  # Each slice is placed on the line it was written on, padding with blank
  # lines, so a backtrace or `__LINE__` in the replay names the line Spinel's
  # run did (#29). A slice that starts on a line already passed — a hook
  # written below its example — follows on the next line instead.
  text = +magic_comments(source)
  line = text.count("\n") + 1
  span.split(",").each do |range|
    first, last = range.split("-").map(&:to_i)
    slice = source.byteslice(first, last - first).force_encoding("UTF-8")
    target = source.byteslice(0, first).count("\n") + 1
    if target > line
      text << "\n" * (target - line)
      line = target
    elsif !text.empty?
      text << "\n"
      line += 1
    end
    text << slice
    line += slice.count("\n")
  end
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
