# The CRuby half of `scripts/verify-passes.rb` (#145): mspec, on a real Ruby,
# running only the examples Spinel passed.
#
# `SPINEL_VERIFY_PASSES` names a file of `file<TAB>description` lines, the
# examples to run; every other example is filtered out, as `-e` would. Each
# one that ran is printed as `VERIFY<TAB>file<TAB>description<TAB>verdict`,
# the verdict empty when Ruby agreed it passes. One Spinel passed that never
# runs here is the script's to report: it has no line at all.
#
# Except under `ruby_bug`. That guard runs its examples everywhere but on the
# CRuby versions that have the bug — so on Spinel, and not here. mspec's
# `report_on` mode (`--report-on ruby_bug`) still evaluates such a block,
# registering its examples as guarded rather than running them; each is
# printed as `RUBY_BUG<TAB>file<TAB>description`, a pass this Ruby cannot
# check.

require "set"
require "mspec/runner/formatters/base"

module SpinelVerify
  PASSES = Hash.new { |hash, file| hash[file] = Set.new }
  File.foreach(ENV.fetch("SPINEL_VERIFY_PASSES"), chomp: true) do |line|
    file, description = line.split("\t", 2)
    PASSES[file] << description
  end

  # The spec file running now, spelled the way Spinel's records spell it.
  def self.file
    MSpec.instance_variable_get(:@file).to_s.sub(%r{\A.*?spec/ruby/}, "spec/ruby/")
  end

  # An `:add` listener: an example defined under a guard this Ruby skips.
  class Guarded
    def add(state)
      print "RUBY_BUG\t#{SpinelVerify.file}\t#{state.description}\n" if MSpec.guarded?
    end

    def register
      MSpec.register :add, self
    end
  end

  # An `:include` filter: an example runs when Spinel passed it.
  class Filter
    def ===(description)
      PASSES.key?(SpinelVerify.file) && PASSES[SpinelVerify.file].include?(description)
    end

    def register
      MSpec.register :include, self
    end
  end
end

class SpinelVerifyFormatter < BaseFormatter
  def register
    super
    SpinelVerify::Filter.new.register
    SpinelVerify::Guarded.new.register
  end

  def before(state = nil)
    super
    @verdict = nil
  end

  def exception(exception)
    super
    # Outside an example — loading the file, `before :all` — there is no
    # example to blame; the passes it stops from running are reported as
    # never run.
    return unless @verdict.nil? && MSpec.current&.state
    first = exception.message.to_s.split("\n").first.to_s
    @verdict = exception.failure? ? "says #{first}" : "raised #{first}"
  end

  def after(state = nil)
    super
    return if state.nil?
    print "VERIFY\t#{SpinelVerify.file}\t#{state.description}\t#{@verdict.to_s.tr("\t\n", "  ")}\n"
  end

  def finish
    # Not mspec's summary: the script reads the lines above, and says what
    # they add up to.
  end
end
