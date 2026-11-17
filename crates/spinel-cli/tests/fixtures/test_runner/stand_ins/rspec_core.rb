# A stand-in for rspec-core, which is not installed here: what `spinel test`
# calls of it (#837, #93), as rspec-core 3 answers it. `RSpec.describe`,
# `it` and `xit` define examples, `RSpec.shared_examples` and
# `it_behaves_like` include examples in a group of their own, an example's
# `pending` marks it pending, and `expect(..).to eq(..)` fails with
# `ExpectationNotMetError`.
# `RSpec::Core::Runner.disable_autorun!` and `.run(args, err, out)` run the
# examples and answer the exit status: the `before(:suite)` hooks first,
# then each example, its end sent to the reporter's listeners as
# `example_passed`, `example_failed` or `example_pending`, and what failed
# and the count written to `out`. `filter_manager.add_location(path, lines)`
# keeps to the examples whose top-level group is in that file and that are
# declared, or in a group declared, at the declaration nearest above each
# line, as rspec-core's location filter does. An example's
# `location_rerun_argument` is the location of it or of the nearest group
# around it in a file that defined a top-level group, as rspec-core's is.
# Each run is logged with the pid that ran it and the examples it ran. The
# real gem's run is checked by hand against CRuby in #837's, #93's and
# #94's pull requests.
module RSpec
  @examples = []
  @shared = {}
  @group_files = []

  class << self
    attr_reader :examples, :shared, :group_files

    def configuration
      @configuration ||= Core::Configuration.new
    end

    def describe(name, &block)
      group = Group.new(name, caller_locations(1, 1).first, nil)
      path = group.metadata[:absolute_file_path]
      group_files << path unless group_files.include?(path)
      group.instance_eval(&block)
    end

    def shared_examples(name, &block)
      shared[name] = block
    end
  end

  module Expectations
    class ExpectationNotMetError < Exception; end

    Target = Struct.new(:actual) do
      def to(expected)
        return if actual == expected

        raise ExpectationNotMetError, "\nexpected: #{expected.inspect}\n     got: #{actual.inspect}\n"
      end
    end
  end

  class Group
    attr_reader :name, :metadata

    def initialize(name, location, parent)
      @name = parent ? "#{parent.name} #{name}" : name
      @metadata = Core.metadata(location, parent&.metadata)
    end

    def it(name, &block)
      RSpec.examples << Core::Example.new("#{@name} #{name}", caller_locations(1, 1).first, metadata, block)
    end

    def xit(name, &block)
      example = Core::Example.new("#{@name} #{name}", caller_locations(1, 1).first, metadata, block)
      example.skip = "Temporarily skipped with xit"
      RSpec.examples << example
    end

    def it_behaves_like(name)
      Group.new("behaves like #{name}", caller_locations(1, 1).first, self)
        .instance_eval(&RSpec.shared.fetch(name))
    end
  end

  module Core
    # An example's or a group's metadata, as far as locations go, its
    # group's as `parent`.
    def self.metadata(location, parent)
      path = location.path.delete_prefix("#{Dir.pwd}/")
      {
        file_path: "./#{path}", line_number: location.lineno, location: "./#{path}:#{location.lineno}",
        absolute_file_path: File.expand_path(location.path), parent: parent,
        rerun_file_path: parent ? parent[:rerun_file_path] : "./#{path}",
      }
    end

    module Pending
      class PendingExampleFixedError < StandardError; end
    end

    # What an example's block runs in.
    class Context
      attr_reader :pending_message

      def expect(actual)
        Expectations::Target.new(actual)
      end

      def eq(expected)
        expected
      end

      def pending(message)
        @pending_message = message
      end
    end

    ExecutionResult = Struct.new(:status, :exception, :pending_message, :run_time)
    Notification = Struct.new(:example)

    class Example
      attr_reader :full_description, :metadata, :execution_result
      attr_accessor :skip

      def initialize(description, location, group, block)
        @full_description = description
        @block = block
        @metadata = Core.metadata(location, group)
        @execution_result = ExecutionResult.new
      end

      # Its metadata, then each group's around it, as rspec-core ascends it.
      def ascending
        list = []
        meta = metadata
        while meta
          list << meta
          meta = meta[:parent]
        end
        list
      end

      def location_rerun_argument
        loaded = RSpec.configuration.loaded_spec_files
        ascending.find { |meta| loaded.include?(meta[:absolute_file_path]) }&.fetch(:location)
      end

      # Its status, as rspec-core gives it: `pending` for an example
      # skipped, or failing after `pending`; `failed` for one raising, or
      # passing after `pending`.
      def run
        result = @execution_result
        start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
        if skip
          result.status = :pending
          result.pending_message = skip
        else
          context = Context.new
          begin
            context.instance_exec(&@block)
            if context.pending_message
              result.status = :failed
              result.exception = Pending::PendingExampleFixedError.new(
                "Expected pending '#{context.pending_message}' to fail. No error was raised."
              )
            else
              result.status = :passed
            end
          rescue StandardError, Expectations::ExpectationNotMetError => e

            if context.pending_message
              result.status = :pending
              result.pending_message = context.pending_message
            else
              result.status = :failed
              result.exception = e
            end
          end
        end
        result.run_time = Process.clock_gettime(Process::CLOCK_MONOTONIC) - start
        result.status
      end
    end

    class Reporter
      def initialize
        @listeners = Hash.new { |hash, key| hash[key] = [] }
      end

      def register_listener(listener, *notifications)
        notifications.each { |name| @listeners[name.to_sym] << listener }
        true
      end

      def notify(name, notification)
        @listeners[name].each { |listener| listener.public_send(name, notification) }
      end
    end

    # rspec-core's: the frames that are not its own.
    class BacktraceFormatter
      def format_backtrace(backtrace, _metadata = {})
        backtrace.reject { |frame| frame.include?("lib/rspec/") || frame.include?("stand_ins/rspec_core") }
      end
    end

    # rspec-core's, as far as locations go: each expanded path's lines.
    class FilterManager
      attr_reader :locations

      def initialize
        @locations = Hash.new { |hash, key| hash[key] = [] }
      end

      def add_location(file_path, line_numbers)
        @locations[File.expand_path(file_path)].concat(line_numbers)
      end

      # The examples to run: each whose top-level group's file has no
      # location, else each that is, or is in a group, declared at the
      # declaration nearest above one of its file's lines.
      def prune(examples)
        declared = Hash.new { |hash, key| hash[key] = [] }
        examples.each do |example|
          example.ascending.each { |meta| declared[meta[:absolute_file_path]] << meta[:line_number] }
        end
        examples.select do |example|
          next true unless @locations.key?(File.expand_path(example.metadata[:rerun_file_path]))

          example.ascending.any? do |meta|
            path = meta[:absolute_file_path]
            @locations.fetch(path, []).any? do |line|
              meta[:line_number] == declared[path].select { |number| number <= line }.max
            end
          end
        end
      end
    end

    class Configuration
      attr_reader :backtrace_formatter, :filter_manager, :loaded_spec_files

      def initialize
        @loaded_spec_files = []
        @before_suite = []
        @backtrace_formatter = BacktraceFormatter.new
        @filter_manager = FilterManager.new
      end

      def reporter
        @reporter ||= Reporter.new
      end

      def prepend_before(scope, &block)
        raise ArgumentError, "only :suite hooks here: #{scope.inspect}" unless scope == :suite

        @before_suite.unshift(block)
      end

      def run_before_suite
        @before_suite.each(&:call)
      end
    end

    module Runner
      def self.disable_autorun!
        @autorun_disabled = true
      end

      def self.run(args, err, out)
        raise "autorun still enabled" unless @autorun_disabled
        raise "paths given: #{args.inspect}" unless args.empty?

        config = RSpec.configuration
        # `load_spec_files`: the files already loaded that defined groups.
        config.loaded_spec_files.concat(RSpec.group_files - config.loaded_spec_files)
        config.run_before_suite
        examples = config.filter_manager.prune(RSpec.examples)
        failed = examples.select do |example|
          status = example.run
          notification = Notification.new(example)
          case status
          when :passed then config.reporter.notify(:example_passed, notification)
          when :pending then config.reporter.notify(:example_pending, notification)
          else
            out.puts "#{example.full_description} failed: #{example.execution_result.exception.message}"
            config.reporter.notify(:example_failed, notification)
          end
          status == :failed
        end
        names = examples.map { |example| example.full_description.tr(" ", "_") }.join(",")
        if (log = ENV["SPINEL_TEST_LOG"])
          File.open(log, "a") { |file| file.write("run #{Process.pid} #{names} #{failed.size}\n") }
        end
        out.puts "#{examples.size} examples, #{failed.size} failures"
        failed.empty? ? 0 : 1
      end
    end
  end
end
