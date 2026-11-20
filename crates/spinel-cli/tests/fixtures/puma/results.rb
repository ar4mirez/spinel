# A minitest reporter that writes how each test of Puma's suite ended, one
# `<mark> <Class>#<test>` line each (`.` passed, `S` skipped, `F` failed, `E`
# an error), to the file `PUMA_SUITE_RESULTS` names: what `suite.txt` holds
# for CRuby's run (`build.rb --suite`) and `tests/puma.rs` compares Spinel's
# with. Loaded with `-r` before Puma's `test/runner`; minitest finds it as a
# plugin, as it finds Puma's own `verbose_progress`, so the suite is run as
# it is and only reported on. Each line is written when its test ends, so
# tests running in parallel cannot interleave within one.
require "minitest"

module Minitest
  def self.plugin_puma_suite_results_init(_options)
    path = ENV["PUMA_SUITE_RESULTS"]
    reporter << PumaSuiteResults.new(path) if path
  end

  class PumaSuiteResults < AbstractReporter
    def initialize(path)
      super()
      @file = File.open(path, "w")
      @file.sync = true
      @lock = Mutex.new
    end

    def record(result)
      @lock.synchronize { @file.write("#{result.result_code} #{result.klass}##{result.name}\n") }
    end

    def report
      @file.close
    end
  end
end

Minitest.extensions << "puma_suite_results" unless Minitest.extensions.include?("puma_suite_results")
