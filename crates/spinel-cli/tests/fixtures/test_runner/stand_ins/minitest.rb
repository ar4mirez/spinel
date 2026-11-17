# A stand-in for minitest, which is not installed here: the part of
# Minitest 5 and 6's API a run goes through, as `shims/test_worker.rb`
# meets it. `Minitest.autorun` registers the `at_exit` that runs the tests;
# `Minitest.run` builds a `CompositeReporter`, sets `Minitest.reporter` for
# `init_plugins` and clears it after, then records a `Result` per test
# method, its first failure saying what it was: a `Skip`, an
# `UnexpectedError` wrapping an exception, else a plain `Assertion`. The
# real gem's run is checked by hand against CRuby in #93's pull request.
module Minitest
  class Assertion < Exception; end
  class Skip < Assertion; end

  class UnexpectedError < Assertion
    attr_accessor :error

    def initialize(error)
      super("Unexpected exception")
      @error = error
    end

    def backtrace
      error.backtrace
    end
  end

  # Minitest's: the frames before the first of its own, else every frame
  # that is not its own; this stand-in's are its own too.
  FILTER = %r{lib/minitest|stand_ins/minitest|internal:warning}

  class << self
    attr_accessor :reporter

    def extensions
      @extensions ||= []
    end

    def init_plugins(options)
      extensions.each { |name| send("plugin_#{name}_init", options) }
    end

    def filter_backtrace(backtrace)
      kept = backtrace.take_while { |frame| frame !~ FILTER }
      kept = backtrace.reject { |frame| frame =~ FILTER } if kept.empty?
      kept.empty? ? backtrace.dup : kept
    end

    def autorun
      at_exit { exit(run ? 0 : 1) }
    end

    def run(_args = [])
      reporter = CompositeReporter.new
      reporter << SummaryReporter.new
      self.reporter = reporter
      init_plugins({})
      self.reporter = nil
      reporter.start
      Test.runnables.each do |klass|
        klass.public_instance_methods(true).grep(/\Atest_/).sort.each do |name|
          reporter.prerecord(klass, name.to_s)
          reporter.record(klass.new(name.to_s).run)
        end
      end
      reporter.report
      reporter.passed?
    end
  end

  class CompositeReporter
    attr_reader :reporters

    def initialize
      @reporters = []
    end

    def <<(reporter)
      @reporters << reporter
    end

    def start = @reporters.each(&:start)
    def prerecord(klass, name) = @reporters.each { |r| r.prerecord(klass, name) }
    def record(result) = @reporters.each { |r| r.record(result) }
    def report = @reporters.each(&:report)
    def passed? = @reporters.all?(&:passed?)
  end

  class SummaryReporter
    def initialize
      @results = []
    end

    def start; end
    def prerecord(_klass, _name); end

    def record(result)
      @results << result
    end

    def passed?
      @results.none? { |r| r.failure && !(Skip === r.failure) }
    end

    def report
      by = @results.group_by { |r| r.failure.class }
      $stdout.puts "#{@results.size} runs, #{(by[Assertion] || []).size} failures, " \
                   "#{(by[UnexpectedError] || []).size} errors, #{(by[Skip] || []).size} skips"
    end
  end

  class Result
    attr_accessor :klass, :name, :failures, :time, :source_location

    def failure
      failures.first
    end
  end

  class Test
    @@runnables = []

    def self.runnables
      @@runnables
    end

    def self.inherited(klass)
      @@runnables << klass
      super
    end

    def initialize(name)
      @name = name
    end

    def run
      failures = []
      start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      begin
        send(@name)
      rescue Assertion => e
        failures << e
      rescue StandardError => e
        failures << UnexpectedError.new(e)
      end
      result = Result.new
      result.klass = self.class.name
      result.name = @name
      result.failures = failures
      result.time = Process.clock_gettime(Process::CLOCK_MONOTONIC) - start
      result.source_location = method(@name).source_location
      result
    end

    def assert_equal(expected, actual)
      return if expected == actual

      raise Assertion, "Expected: #{expected.inspect}\n  Actual: #{actual.inspect}"
    end

    def skip(message = "Skipped, no message given")
      raise Skip, message
    end
  end
end
