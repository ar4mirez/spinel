# A stand-in for rspec-core, which is not installed here: what `spinel test`
# calls of it (#837) — `RSpec.describe` and `it` to define examples, and
# `RSpec::Core::Runner.disable_autorun!` and `.run(args, err, out)` to run
# them, answering the exit status — logging each run with the pid that ran
# it and the examples it ran. The real gem's run is checked by hand against
# CRuby in #837's pull request.
module RSpec
  @examples = []

  class << self
    attr_reader :examples

    def describe(name, &block)
      Group.new(name).instance_eval(&block)
    end
  end

  class Group
    def initialize(name)
      @name = name
    end

    def it(name, &block)
      RSpec.examples << ["#{@name} #{name}", block]
    end
  end

  module Core
    module Runner
      def self.disable_autorun!
        @autorun_disabled = true
      end

      def self.run(args, err, out)
        raise "autorun still enabled" unless @autorun_disabled
        raise "paths given: #{args.inspect}" unless args.empty?
        failed = RSpec.examples.reject do |name, block|
          block.call
          true
        rescue StandardError => e
          err.puts "#{name} failed: #{e.message}"
          false
        end
        names = RSpec.examples.map { |name, _| name.tr(" ", "_") }.join(",")
        File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") do |log|
          log.write("run #{Process.pid} #{names} #{failed.size}\n")
        end
        out.puts "#{RSpec.examples.size} examples, #{failed.size} failures"
        failed.empty? ? 0 : 1
      end
    end
  end
end
