# How mspec reports on Spinel (#145): three outcomes, not two.
#
# An example *passes*, *fails* — an expectation was not met — or is
# *blocked*: Spinel cannot run it yet. Blocked is the column `spec/harness`
# kept for five phases, and it keeps the failure column meaning one thing — a
# disagreement with Ruby — so CI can demand zero of them while most of the
# corpus is still out of reach.
#
# Two things block an example:
#
# - A *refusal*: something the VM declines to guess at, or an example that
#   runs past its instruction budget. `__refusal_boundary__` ends the example
#   and answers the reason, and no `rescue` in the spec or in mspec sees it.
# - Any exception other than an unmet expectation — a `NoMethodError` for a
#   method `core/` does not have yet, most often. That is the harness's rule
#   too, and the one place a real disagreement could hide: an example that
#   should pass but raises. `scripts/verify-passes.rb` checks the other
#   direction, every pass against CRuby.

require "mspec/runner/mspec"
require "mspec/runner/formatters/base"

module SpinelRunner
  # Per example, as the harness had it per evaluation: enough for any example
  # in the corpus, and a loop Spinel gets wrong is reported rather than hung.
  BUDGET = 50_000_000

  # Loading a file runs every group in it, so it is a boundary for refusals
  # outside any example but has no budget of its own.
  UNLIMITED = 2**62 - 1

  class << self
    # Called by the formatter: what blocked the example running now, if
    # anything did.
    attr_accessor :formatter
  end
end

class << MSpec
  alias_method :__spinel_protect__, :protect

  # Every example, hook and cleanup mspec runs comes through here; each runs
  # inside a refusal boundary.
  def protect(location, &block)
    result = nil
    finished = false
    loading = location.to_s.start_with?("loading ")
    budget = loading ? SpinelRunner::UNLIMITED : SpinelRunner::BUDGET
    reason = __refusal_boundary__(budget) do
      result = __spinel_protect__(location, &block)
      finished = true
    end
    return result if finished
    SpinelRunner.formatter&.blocked(location, reason)
    # A refusal while a file loads can leave a group open; what is left of the
    # file does not run.
    register_current(nil) if loading
    false
  end
end

# A fixture that stops part way is left where it stopped (#145). This is
# `spec/harness`'s rule, kept for the same reason: fixtures raise part way
# constantly — on a `Set`, a `Struct`, a `Kernel.instance_method` this VM
# does not have yet — and almost always past what most of their examples
# need. Ending the whole spec file there, as Ruby would, blocks hundreds of
# examples that run fine against what the fixture did define.
#
# Only `fixtures/` and `shared/` files are lenient, never the spec file
# itself; the stop is reported; and the heap is marked partial, so
# `defined?` refuses rather than answering `nil` for a name the unfinished
# part would have defined. `scripts/verify-passes.rb` re-runs every pass on
# CRuby, which is what catches an example passing off a half-built fixture.
module Kernel
  class << self
    alias_method :__spinel_require__, :__require__

    def __require__(name)
      path = File.__path__(name)
      # A spec file's own `require` of a library this VM cannot load yet —
      # `stringio`, `date` — at its top level, outside any group: the
      # harness ignored these, and the examples that use the library block
      # on the missing constant instead. Inside an example, `require` raises
      # as Ruby's does, which is what the `require` specs check.
      if MSpec.current.nil? && !path.start_with?("/", ".")
        begin
          return __spinel_require__(path)
        rescue LoadError => e
          SpinelRunner.formatter&.fixture_stopped(path, "#{e.class}: #{e.message}")
          return false
        end
      end
      return __spinel_require__(path) unless path.match?(%r{spec/ruby/.*/(fixtures|shared)/})
      result = nil
      reason = __refusal_boundary__(SpinelRunner::UNLIMITED) do
        result = begin
          __spinel_require__(path)
        rescue StandardError, ScriptError => e
          SpinelRunner.formatter&.fixture_stopped(path, "#{e.class}: #{e.message.to_s.split("\n").first.to_s}")
          __mark_partial__
          true
        end
      end
      return result unless String === reason && result.nil?
      SpinelRunner.formatter&.fixture_stopped(path, reason)
      __mark_partial__
      true
    end
  end
end

class SpinelFormatter < BaseFormatter
  MILESTONES = "https://github.com/ar4mirez/spinel/milestones"

  def initialize(out = nil)
    super
    @passed = 0
    @failed = []
    @blocked = Hash.new(0)
    # Files whose loading a refusal stopped: their examples never registered,
    # so they are named here rather than counted.
    @unloaded = []
    @fixtures = {}
    @reason = nil
    @files = {}
    # `SPINEL_SPEC_LIST=1` prints one `LIST<TAB>outcome<TAB>file<TAB>description`
    # line per example after the report: what `scripts/verify-passes.rb`
    # re-runs on CRuby. Printed rather than written to a file because writing
    # a file is #41's.
    @list = ENV["SPINEL_SPEC_LIST"]
    @listed = []
    SpinelRunner.formatter = self
  end

  def __list__(outcome, state)
    return unless @list
    file = MSpec.instance_variable_get(:@file).to_s.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
    @listed << "#{outcome}\t#{file}\t#{state.description}"
  end

  def register
    super
    MSpec.register :load, self
  end

  def load
    file = MSpec.instance_variable_get(:@file)
    @files[file] = true
    # `SPINEL_SPEC_TRACE=1` names each file as it starts, for finding the one
    # that takes the process down.
    $stderr.puts "loading #{file}" if ENV["SPINEL_SPEC_TRACE"]
  end

  def before(state = nil)
    super
    $stderr.puts "  #{state.description}" if state && ENV["SPINEL_SPEC_TRACE"] == "2"
    @reason = nil
    @failure_message = nil
  end

  def fixture_stopped(path, reason)
    @fixtures[path.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")] ||= reason
  end

  # A refusal. In `before :all`, nothing in the group runs, so every example
  # in it is blocked; anywhere else it is the current example's.
  def blocked(location, reason)
    if location.to_s.start_with?("loading ")
      file = location.to_s.delete_prefix("loading ").sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
      @unloaded << "#{file}: #{reason}"
    elsif location == "before :all" && MSpec.current
      MSpec.current.examples.size.times { @blocked[reason] += 1 }
    else
      @reason ||= reason
    end
  end

  def exception(exception)
    if exception.failure?
      @failure_message ||= exception.message.to_s.split("\n").first.to_s
      return
    end
    error = exception.exception
    reason = "#{error.class}: #{error.message.to_s.split("\n").first.to_s}"
    if exception.description.start_with?("An exception occurred during: loading ")
      file = exception.description.split("\n").first.to_s
                      .delete_prefix("An exception occurred during: loading ")
                      .sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
      @unloaded << "#{file}: #{reason}"
    elsif exception.description.start_with?("An exception occurred during: before :all") && MSpec.current
      MSpec.current.examples.size.times { @blocked[reason] += 1 }
    else
      @reason ||= reason
    end
  end

  def after(state = nil)
    return super if state.nil?
    if @reason
      @blocked[@reason] += 1
      __list__("blocked", state)
      print "B"
    elsif @failure_message
      __list__("failed", state)
      file = MSpec.instance_variable_get(:@file).to_s.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
      @failed << "#{file} #{state.description}: #{@failure_message}"
      print "F"
    else
      @passed += 1
      __list__("passed", state)
      print "."
    end
    super
  end

  def finish
    blocked = @blocked.values.sum
    examples = @passed + @failed.size + blocked
    tagged = @tally.counter.tagged
    print "\n\n"
    unless @failed.empty?
      print "failed (#{@failed.size}):\n"
      @failed.each { |line| print "  #{line}\n" }
      print "\n"
    end
    print "#{@files.size} files · #{examples} examples · #{@passed} passed · " \
          "#{@failed.size} failed · #{blocked} blocked · #{tagged} skipped · #{@timer.format.split(" ")[2]}s\n"
    unless @fixtures.empty?
      print "\nfixtures that stopped part way (#{@fixtures.size}):\n"
      @fixtures.each { |path, reason| print "  #{path}: #{reason}\n" }
    end
    unless @unloaded.empty?
      print "\nstopped while loading (#{@unloaded.size}):\n"
      @unloaded.each { |line| print "  #{line}\n" }
    end
    unless @blocked.empty?
      print "\nblocked by, most examples first (#{MILESTONES}):\n"
      @blocked.sort_by { |reason, count| [-count, reason] }.first(20).each do |reason, count|
        print "#{count.to_s.rjust(7)}  #{reason}\n"
      end
      print "  ... and #{@blocked.size - 20} more reasons\n" if @blocked.size > 20
    end
    MSpec.register_exit(@failed.empty? ? 0 : 1)
    __write_list__ if @list
  end

  def __write_list__
    @listed.each { |line| print "LIST\t#{line}\n" }
  end
end
