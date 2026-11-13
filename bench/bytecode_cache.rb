# The bytecode cache (#40): a project of 500 required files, run cold — no
# `.spinel/bytecode/`, so every file is parsed, compiled and written — and
# warm, where every `require` reads its file's bytecode back instead. The
# difference is what the cache saves a second run.
#
#   scripts/bench.sh bytecode_cache
#   target/release/spinel run bench/bytecode_cache.rb
#   ruby --yjit bench/bytecode_cache.rb
#
# Either way it times the release `spinel` (or $SPINEL) as a child process,
# since what is measured is a whole run; on Ruby it also times `ruby --yjit`
# on the same project for comparison, which has no cache to warm. Numbers go
# in the pull request, not here (`docs/roadmap.md`, rule 7).

require "tmpdir"

FILES = 500
RUNS = 7
SPINEL = ENV.fetch("SPINEL") { File.expand_path("../target/release/spinel", __dir__) }

# One required file: a module with a class, a few methods, blocks, string
# and symbol literals, a regexp and a constant — an ordinary library file.
def library(n)
  <<~RUBY
    # frozen_string_literal: true

    module Lib#{n}
      VERSION = "1.#{n}.0"
      NAMES = %w[alpha beta gamma delta epsilon].freeze

      class Item
        attr_reader :name, :size

        def initialize(name, size = #{n % 7})
          @name = name
          @size = size
        end

        def to_s
          "\#{name}(\#{size})"
        end

        def <=>(other)
          [size, name] <=> [other.size, other.name]
        end
      end

      def self.items
        NAMES.each_with_index.map { |name, i| Item.new(name, (i * #{n}) % 11) }
      end

      def self.summary
        items.sort.map(&:to_s).select { |s| s.match?(/a/) }.join(", ")
      end

      def self.checksum
        items.sum { |item| item.size * item.name.length } + #{n}
      end

      def self.classify(value)
        case value
        when Integer then value.even? ? :even : :odd
        when String then value.empty? ? :blank : :text
        when nil then :none
        else :other
        end
      end
    end
  RUBY
end

def time_ms
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
end

def median(times)
  times.sort[times.length / 2]
end

def run(dir, *command)
  ok = nil
  elapsed = Dir.chdir(dir) { time_ms { ok = system(*command) } }
  abort "failed: #{command.join(" ")}" unless ok
  elapsed
end

Dir.mktmpdir("spinel-bench-bytecode-cache") do |dir|
  Dir.mkdir(File.join(dir, "lib"))
  FILES.times { |n| File.write(File.join(dir, "lib", "lib#{n}.rb"), library(n)) }
  main = FILES.times.map { |n| "require_relative \"lib/lib#{n}\"\n" }.join
  main += "total = 0\n"
  main += FILES.times.map { |n| "total += Lib#{n}.checksum\n" }.join
  main += "File.write(File.join(__dir__, \"total\"), total.to_s)\n"
  File.write(File.join(dir, "main.rb"), main)
  # What `spinel install` leaves, which is what makes this a project.
  Dir.mkdir(File.join(dir, ".spinel"))
  Dir.mkdir(File.join(dir, ".spinel", "gem_home"))
  cache = File.join(dir, ".spinel", "bytecode")

  clear = lambda do
    next unless File.directory?(cache)
    Dir.children(cache).each { |name| File.delete(File.join(cache, name)) }
    Dir.rmdir(cache)
  end

  cold = RUNS.times.map do
    clear.call
    run(dir, SPINEL, "run", "main.rb")
  end
  written = Dir.children(cache).length
  warm = RUNS.times.map { run(dir, SPINEL, "run", "main.rb") }
  total = File.read(File.join(dir, "total"))

  puts "#{FILES} required files, #{written} cache files, total #{total}"
  puts "spinel cold (parse, compile, write): #{median(cold).round(1)} ms (median of #{RUNS})"
  puts "spinel warm (read the cache):        #{median(warm).round(1)} ms (median of #{RUNS})"
  puts "warm is #{(median(cold) / median(warm)).round(2)}x faster"
  if RUBY_ENGINE == "ruby"
    yjit = RUNS.times.map { run(dir, RbConfig.ruby, "--yjit", "main.rb") }
    puts "ruby --yjit:                         #{median(yjit).round(1)} ms (median of #{RUNS})"
  end
end
