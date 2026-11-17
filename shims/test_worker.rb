# Spinel's test worker events (docs/test-runner.md, "Execution"): loaded
# before `shims/test_runner.rb`, as `-r` loads a library before a program.
# Each worker writes its events to the pipe back to the runner, one JSON
# line per event:
#
#   {"event":"done","file":..,"error":..}   a file it was given is loaded
#   {"event":"pass|fail|error|skip","file":..,"line":..,"name":..,"ms":..,
#    "message":..,"backtrace":[..],"rerun":..}
#                                           a test or an example ended
#
# `file` and `line` are where the test or example is defined; `rerun` is
# the `path` or `path:LINE` that `spinel test` runs it again with, which a
# test defined outside the file that runs it (a Minitest module's, an RSpec
# shared example's) cannot be found by.
#
# A Minitest reporter and an RSpec formatter write the second kind. Each
# event is one `write`, under a lock, on a pipe that is `sync`, so a line is
# never split across two flushes nor mixed with another.
#
# What counts as what is each framework's own: Minitest by a result's first
# failure, as its `result_code` and its summary count them (`Skip` a skip,
# `UnexpectedError` an error, any other assertion a failure); RSpec by an
# example's status (`passed`, `failed`, `pending` for pending and skipped
# examples). RSpec has no errors: an exception in an example is a failure,
# as `rspec` counts it.

require "json"

module Spinel
  module TestWorker
    @events = nil
    @lock = Mutex.new
    # How many failures and errors this worker has reported.
    @failed = 0
    # The file each Minitest test class was defined by, by name.
    @homes = {}
    # The files this worker was given, by their expanded paths.
    @given = {}

    class << self
      attr_reader :failed
    end

    # Write events to `io` from now on.
    def self.start(io)
      io.sync = true
      @events = io
    end

    # One event, as one line in one write.
    def self.emit(fields)
      line = JSON.generate(fields) + "\n"
      @lock.synchronize { @events.write(line) }
    end

    # A test's or an example's end.
    def self.result(event, file, line, name, seconds, message, backtrace, rerun)
      @failed += 1 if event == "fail" || event == "error"
      emit(
        "event" => event, "file" => file && relative(file.to_s), "line" => line,
        "name" => text(name), "ms" => (seconds.to_f * 1000).round(3),
        "message" => message && text(message),
        "backtrace" => backtrace.map { |frame| relative(text(frame)) },
        "rerun" => rerun && relative(rerun)
      )
    end

    # Where test `name` of class `klass`, defined at `file`:`line`, runs
    # again: that file at that line when it is a file given and the class
    # defines the test itself, else the whole file given that defined the
    # class; nil for a class no file given defined.
    def self.minitest_rerun(klass, name, file, line)
      given = file && line && @given[File.expand_path(file)]
      return "#{given}:#{line}" if given && own_test?(klass, name)

      @homes[klass.to_s]
    end

    # Whether the test class named `klass` defines method `name` itself,
    # rather than inheriting it or including it from a module.
    def self.own_test?(klass, name)
      runnable = runnables.find { |candidate| candidate.name == klass.to_s }
      return false unless runnable && runnable.method_defined?(name)

      runnable.instance_method(name).owner == runnable
    end

    # A path under the current directory as the test files are named, from
    # it: `test/a_test.rb`, not `/home/me/app/test/a_test.rb` or
    # `./test/a_test.rb`.
    def self.relative(path)
      path.delete_prefix("#{Dir.pwd}/").delete_prefix("./")
    end

    # A string JSON can carry: UTF-8, with what is not valid replaced.
    def self.text(value)
      value.to_s.encode(Encoding::UTF_8, invalid: :replace, undef: :replace).scrub
    rescue EncodingError
      value.to_s.b.force_encoding(Encoding::UTF_8).scrub
    end

    # The Minitest test classes defined so far.
    def self.runnables
      return [] unless defined?(::Minitest::Runnable) && ::Minitest::Runnable.respond_to?(:runnables)

      ::Minitest::Runnable.runnables.dup
    end

    # Add the reporter to the reporters `Minitest.run` makes, after its own
    # plugins have had theirs added, so a plugin that replaces the default
    # reporters (Rails') leaves this one, and take out the default ones,
    # whose report the runner's replaces. Not a plugin by name: adding to
    # `Minitest.extensions` before it loads its plugins would stop it
    # loading them on the Minitest 5 releases that load only into an empty
    # list.
    #
    # `files` is what the worker loaded: each path, the line asked for (nil
    # for all of it), and the test classes that appeared when it was
    # required. When any has a line, only the tests at it run of the
    # classes that file defined.
    # ponytail: a plugin's own printing reporter (Rails' `TestUnitReporter`)
    # still prints, on stderr under `--reporter json` or `junit` (#95);
    # taking it out of the run too, as the `SummaryReporter` is, is the
    # upgrade.
    def self.minitest(files = [])
      return unless defined?(::Minitest) && ::Minitest.respond_to?(:init_plugins)

      files.each do |path, _, classes|
        @given[File.expand_path(path)] = path
        classes.each { |klass| @homes[klass.name] ||= path if klass.name }
      end
      ours = MinitestReporter.new
      ::Minitest.singleton_class.prepend(Module.new do
        define_method(:init_plugins) do |options|
          super(options)
          if reporter
            defaults = %i[SummaryReporter ProgressReporter].filter_map do |name|
              ::Minitest.const_get(name) if ::Minitest.const_defined?(name)
            end
            if reporter.respond_to?(:reporters)
              reporter.reporters.reject! { |r| defaults.any? { |klass| klass === r } }
            end
            reporter << ours
          end
        end
      end)
      only(files)
    end

    # Run only the tests at the line asked for, of the classes the files
    # with a line defined; every test of any other class.
    def self.only(files)
      return if files.none? { |_, line, _| line } || !defined?(::Minitest::Test)

      restricted = []
      chosen = []
      files.each do |path, line, classes|
        next unless line

        restricted.concat(classes)
        chosen.concat(at_line(File.expand_path(path), line))
      end
      ::Minitest::Test.singleton_class.prepend(Module.new do
        define_method(:runnable_methods) do
          names = super()
          return names unless restricted.include?(self)

          names.select { |name| chosen.include?([self, name.to_s]) }
        end
      end)
    end

    # `[class, method]` for each test whose method starts nearest above or
    # at `wanted`, in `path`: as `rspec` finds the declaration nearest above
    # a line, and as a test's `source_location` names it.
    def self.at_line(path, wanted)
      tests = runnables.flat_map do |klass|
        next [] unless klass.respond_to?(:methods_matching)

        klass.methods_matching(/\Atest_/).filter_map do |name|
          file, line = klass.instance_method(name).source_location
          [klass, name.to_s, line] if file && line && File.expand_path(file) == path
        end
      end
      nearest = tests.map { |_, _, line| line }.select { |line| line <= wanted }.max
      tests.select { |_, _, line| line == nearest }.map { |klass, name, _| [klass, name] }
    end

    # Listen for each example's end, once RSpec has set up its own
    # formatters: an RSpec with none configured adds its default only when
    # no other is there, so this one, added before, would replace it.
    # ponytail: `--dry-run` runs no suite hooks, so it reports no results;
    # a hook on the reporter's `start` would catch it too.
    #
    # `locations` are the files given with a line, each `[path, line]`,
    # added to RSpec's location filter as `rspec path:line` adds them: only
    # the examples declared nearest above that line run of that file.
    def self.rspec(locations = [])
      config = ::RSpec.configuration
      locations.each { |path, line| config.filter_manager.add_location(path, [line]) }
      config.prepend_before(:suite) do
        config.reporter.register_listener(
          RSpecFormatter.new, :example_passed, :example_failed, :example_pending
        )
      end
    end

    # A Minitest reporter, as `Minitest::AbstractReporter` defines one.
    class MinitestReporter
      def start; end
      def prerecord(_klass, _name); end
      def report; end

      # False once a test failed or raised, as Minitest's own summary
      # answers, so `minitest/autorun` exits 1 without it.
      def passed?
        TestWorker.failed.zero?
      end

      def synchronize
        yield
      end

      def record(result)
        failure = result.failure
        event =
          if failure.nil? then "pass"
          elsif ::Minitest::Skip === failure then "skip"
          elsif ::Minitest::UnexpectedError === failure then "error"
          else "fail"
          end
        file, line = result.source_location if result.respond_to?(:source_location)
        klass = result.respond_to?(:klass) ? result.klass : result.class.name
        message, backtrace =
          case event
          when "error" then ["#{failure.error.class}: #{failure.error.message}", failure.backtrace]
          when "fail" then [failure.message, failure.backtrace]
          when "skip" then [failure.message, nil]
          end
        backtrace = backtrace ? ::Minitest.filter_backtrace(backtrace) : []
        TestWorker.result(
          event, file, line, "#{klass}##{result.name}", result.time, message, backtrace,
          TestWorker.minitest_rerun(klass, result.name, file, line)
        )
      end
    end

    # An RSpec formatter's example notifications. An example runs again at
    # its `location_rerun_argument`, as RSpec's own summary prints it: its
    # own location, or that of the group that included it from a spec file
    # (`it_behaves_like`).
    class RSpecFormatter
      def example_passed(notification)
        example(notification.example, "pass")
      end

      def example_failed(notification)
        exception = notification.example.execution_result.exception
        backtrace = ::RSpec.configuration.backtrace_formatter
          .format_backtrace(exception.backtrace || [], notification.example.metadata)
        example(notification.example, "fail", message(exception), backtrace)
      end

      def example_pending(notification)
        example(notification.example, "skip", notification.example.execution_result.pending_message)
      end

      private

      # The exception as RSpec's failure report puts it: its class, then its
      # message, leaving out a class whose name says `RSpec`
      # (`ExpectationNotMetError`, a pending example that passed).
      def message(exception)
        name = exception.class.name.to_s
        name.include?("RSpec") ? exception.message : "#{name}: #{exception.message}"
      end

      def example(example, event, message = nil, backtrace = [])
        metadata = example.metadata
        TestWorker.result(
          event, metadata[:file_path], metadata[:line_number], example.full_description,
          example.execution_result.run_time, message, backtrace, example.location_rerun_argument
        )
      end
    end
  end
end
