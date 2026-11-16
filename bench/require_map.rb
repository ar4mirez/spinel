# The require map (#66): a project of 100 gems, each with a file that
# requires five more of its own, and a program that puts every gem's `lib/`
# on `$LOAD_PATH` as `Bundler.setup` does, then requires each gem and eight
# standard libraries. Each `require` looks along a load path of 100 gem
# directories before the standard library. It is run without a require map,
# where each of those directories is looked at on disk, and with one, as
# `spinel install` writes it, which passes over those that do not have the
# file. The bytecode cache is warm for both, so what differs is the search.
#
#   scripts/bench.sh require_map
#   target/release/spinel run bench/require_map.rb
#   ruby --yjit bench/require_map.rb
#
# Either way it times the release `spinel` (or $SPINEL) as a child process,
# since what is measured is a whole run; on Ruby it also times `ruby --yjit`
# on the same program for comparison, which has no map. The map is written
# with `spinel require-map`, the hidden command that writes what `spinel
# install` writes for directories given. Numbers go in the pull request, not
# here (`docs/roadmap.md`, rule 7).

require "tmpdir"

GEMS = 100
FILES = 5
RUNS = 5
STDLIB = %w[optparse erb time fileutils tmpdir securerandom shellwords uri].freeze
SPINEL = ENV.fetch("SPINEL") { File.expand_path("../target/release/spinel", __dir__) }

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

Dir.mktmpdir("spinel-bench-require-map") do |dir|
  dir = File.realpath(dir)
  libs = GEMS.times.map do |g|
    lib = File.join(dir, "gems", "gem#{g}-1.0.0", "lib")
    Dir.mkdir(File.join(dir, "gems")) unless File.directory?(File.join(dir, "gems"))
    Dir.mkdir(File.dirname(lib))
    Dir.mkdir(lib)
    Dir.mkdir(File.join(lib, "gem#{g}"))
    File.write(File.join(lib, "gem#{g}.rb"),
               FILES.times.map { |f| "require \"gem#{g}/part#{f}\"\n" }.join +
               "module Gem#{g}\n  def self.parts = #{FILES}\nend\n")
    FILES.times do |f|
      File.write(File.join(lib, "gem#{g}", "part#{f}.rb"),
                 "module Gem#{g}\n  module Part#{f}\n    VALUE = #{g * FILES + f}\n  end\nend\n")
    end
    lib
  end
  main = "$LOAD_PATH.unshift(*#{libs.inspect})\n"
  main += GEMS.times.map { |g| "require \"gem#{g}\"\n" }.join
  main += STDLIB.map { |name| "require #{name.inspect}\n" }.join
  main += "total = #{GEMS.times.map { |g| "Gem#{g}.parts" }.join(" + ")}\n"
  main += "File.write(File.join(__dir__, \"total\"), total.to_s)\n"
  File.write(File.join(dir, "main.rb"), main)
  # What `spinel install` leaves, which is what makes this a project.
  Dir.mkdir(File.join(dir, ".spinel"))
  Dir.mkdir(File.join(dir, ".spinel", "gem_home"))
  map = File.join(dir, ".spinel", "require_map.bin")

  # Warm the bytecode cache, so neither side parses.
  run(dir, SPINEL, "run", "main.rb")
  without = RUNS.times.map { run(dir, SPINEL, "run", "main.rb") }
  run(dir, SPINEL, "require-map", map, *libs)
  with = RUNS.times.map { run(dir, SPINEL, "run", "main.rb") }
  total = File.read(File.join(dir, "total"))

  puts "#{GEMS} gems of #{FILES + 1} files and #{STDLIB.length} standard libraries " \
       "required (#{GEMS * (FILES + 1) + STDLIB.length} requires), total #{total}, " \
       "map #{File.size(map)} bytes"
  puts "spinel without a map:      #{median(without).round(1)} ms (median of #{RUNS})"
  puts "spinel with the map:       #{median(with).round(1)} ms (median of #{RUNS})"
  puts "with the map is #{(median(without) / median(with)).round(2)}x faster"
  if RUBY_ENGINE == "ruby"
    yjit = RUNS.times.map { run(dir, RbConfig.ruby, "--yjit", "main.rb") }
    puts "ruby --yjit:               #{median(yjit).round(1)} ms (median of #{RUNS})"
  end
end
