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
require_relative "report"

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

# mspec writes a failure's values with `pretty_inspect`, which is `pp`'s, and
# `pp` is a library this VM cannot load yet. `inspect` is what it prints for
# every value small enough to fit a line, which a failure message's are.
module Kernel
  def pretty_inspect
    "#{inspect}\n"
  end
end unless Kernel.method_defined?(:pretty_inspect)

class SpinelFormatter < BaseFormatter
  def initialize(out = nil)
    super
    # One record per outcome, `[kind, file, description, detail]` — the kinds
    # are listed in `spec/spinel/report.rb`. `SpinelReport` makes the report
    # from them, and `SPINEL_SPEC_LIST=1` prints them instead, for
    # `scripts/spec-report.sh` to merge across processes.
    @records = []
    @list = ENV["SPINEL_SPEC_LIST"]
    @fixtures = {}
    SpinelRunner.formatter = self
  end

  def register
    super
    MSpec.register :load, self
    MSpec.register :tagged, self
  end

  def __file__
    MSpec.instance_variable_get(:@file).to_s.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
  end

  def __record__(kind, description = "", detail = "", file = __file__)
    @records << [kind, file, description, detail.to_s.tr("\t\n", "  ")]
  end

  def load
    __record__("file")
    # `SPINEL_SPEC_TRACE=1` names each file as it starts, for finding the one
    # that takes the process down.
    $stderr.puts "loading #{MSpec.instance_variable_get(:@file)}" if ENV["SPINEL_SPEC_TRACE"]
  end

  def tagged(state)
    __record__("skipped", state.description)
  end

  def before(state = nil)
    super
    $stderr.puts "  #{state.description}" if state && ENV["SPINEL_SPEC_TRACE"] == "2"
    @reason = nil
    @failure_message = nil
  end

  def fixture_stopped(path, reason)
    path = path.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
    return if @fixtures.key?(path)
    @fixtures[path] = true
    __record__("fixture", "", reason, path)
  end

  # Every example of the group a `before :all` was running for: none of them
  # will run.
  def __block_group__(reason)
    MSpec.current.examples.each { |example| __record__("blocked", example.description, reason) }
  end

  # Whether mspec is still running a `describe` body to collect its examples.
  # One that stops there — a `guard` whose lambda raises, a refusal — is not
  # processed at all, so without this its examples vanish from every count.
  def __parsing__
    MSpec.current && !MSpec.current.instance_variable_get(:@parsed)
  end

  # The examples it collected before it stopped are blocked; the ones after
  # were never defined, and the group is reported as stopped while loading.
  def __stop_group__(reason)
    __record__("unloaded", "", "#{MSpec.current.description}: #{reason}")
    __block_group__(reason)
  end

  # A refusal. While a file loads, the rest of it does not; in `before :all`,
  # nothing in the group runs; anywhere else it is the current example's.
  def blocked(location, reason)
    if location.to_s.start_with?("loading ")
      __record__("unloaded", "", reason)
    elsif __parsing__
      __stop_group__(reason)
    elsif location == "before :all" && MSpec.current
      __block_group__(reason)
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
    reason = "#{error.class}: #{error.message.to_s.split("\n").first}"
    where = exception.description.split("\n").first.to_s
    if where.start_with?("An exception occurred during: loading ")
      __record__("unloaded", "", reason)
    elsif __parsing__
      __stop_group__(reason)
    elsif where.start_with?("An exception occurred during: before :all") && MSpec.current
      __block_group__(reason)
    else
      @reason ||= reason
    end
  end

  def after(state = nil)
    return super if state.nil?
    if @reason
      __record__("blocked", state.description, @reason)
      print "B" unless @list
    elsif @failure_message
      __record__("failed", state.description, @failure_message)
      print "F" unless @list
    else
      __record__("passed", state.description)
      print "." unless @list
    end
    super
  end

  def finish
    @records.concat(SpinelReport.tag_problems(@records, File.expand_path("../..", __dir__)))
    failed = @records.count { |r| r[0] == "failed" || r[0] == "tag" }
    if @list
      @records.each { |record| print "LIST\t#{record.join("\t")}\n" }
    else
      print "\n\n"
      print SpinelReport.report(@records, "#{@timer.format.split(" ")[2]}s")
    end
    MSpec.register_exit(failed.zero? ? 0 : 1)
  end
end
