#!/usr/bin/env ruby
# frozen_string_literal: true

# The anti-false-pass check.
#
# A spec runner's pass count is only worth anything if a pass means what it
# says. This script takes every example Spinel passes and runs it again, under
# the same mspec, on a real Ruby. If Ruby raises where Spinel did not, or an
# expectation does not hold, then Spinel passed an example it had no right to
# — which is strictly worse than reporting it blocked.
#
#   scripts/verify-passes.rb [dir-or-file ...]     # default: language
#
# Paths are as `scripts/spec.sh` takes them. Spinel's side is
# `scripts/spec.sh --list`; Ruby's is `spec/verify.mspec`, which runs only
# the examples Spinel passed, each file whole, as mspec runs it — hooks,
# fixtures and all. Before mspec ran on Spinel (#145) this script sliced each
# example out of its file and replayed it under a shim; running the real thing
# on both sides is what made that unnecessary.
#
# A pass Ruby never runs is reported too. It means the two disagree about
# which examples exist — a guard answered differently, a description built
# from an `inspect` that differs — and a pass nobody checked is not one. The
# exception is an example under `ruby_bug`, which runs on Spinel and not on
# a CRuby that has the bug; those are counted, and named by
# `spec/spinel/verify.rb`.

require "etc"
require "open3"
require "rbconfig"
require "tmpdir"

ROOT = File.expand_path("..", __dir__)
MSPEC = ENV.fetch("MSPEC_DIR", File.join(ROOT, "spec", "mspec"))
abort "mspec is not checked out at #{MSPEC}: git submodule update --init spec/mspec" \
  unless File.exist?(File.join(MSPEC, "bin", "mspec-run"))

paths = ARGV.empty? ? ["language"] : ARGV
listing, status = Open3.capture2(File.join(ROOT, "scripts", "spec.sh"), "--list", *paths)
# The listing is read even when an example failed: this script's job is the
# ones that *passed*, and refusing a run with failures in it would switch the
# check off exactly when the suite is most in flux. A process that stopped is
# another matter — its passes are missing — and `spec.sh` has said which.
abort "scripts/spec.sh --list did not finish (#{status})" unless status.success? || status.exitstatus == 1

passes = []
chunks = Hash.new { |hash, index| hash[index] = [] }
listing.each_line do |line|
  kind, *fields = line.chomp.split("\t", 4)
  case kind
  when "passed" then passes << fields.first(2)
  when "chunk" then chunks[fields[0]] << File.join(ROOT, fields[1])
  end
end
abort "no passing examples to verify — that is itself a regression" if passes.empty?

# Ruby runs the files in the processes Spinel ran them in, in the same order,
# a few at a time. A file can see what an earlier one in its process left
# behind — `SpecEvaluate.desc` is global, and an example's description is
# built from it — so a different grouping is a different run.
verdicts = {}
bugs = {}
Dir.mktmpdir("verify-passes") do |dir|
  list = File.join(dir, "passes.tsv")
  File.write(list, passes.map { |pass| pass.join("\t") + "\n" }.join)
  env = { "SPINEL_VERIFY_PASSES" => list, "RUBY_EXE" => RbConfig.ruby }
  wanted = passes.map(&:first).uniq.map { |file| File.join(ROOT, file) }
  queue = Queue.new
  chunks.each_value { |files| queue << files if files.intersect?(wanted) }
  queue.close
  outputs = Array.new(Etc.nprocessors) do
    Thread.new do
      out = +""
      while (files = queue.pop)
        output, = Open3.capture2(env, RbConfig.ruby, File.join(MSPEC, "bin", "mspec-run"),
                                 "-B", File.join(ROOT, "spec", "verify.mspec"), *files)
        out << output
      end
      out
    end
  end.map(&:value)
  outputs.join.each_line do |line|
    kind, file, description, verdict = line.chomp.split("\t", 4)
    verdicts[[file, description]] = verdict.to_s if kind == "VERIFY"
    bugs[[file, description]] = true if kind == "RUBY_BUG"
  end
end

wrong = []
unchecked = 0
passes.each do |pass|
  verdict = verdicts[pass]
  if verdict.nil? && bugs[pass]
    unchecked += 1
  elsif verdict.nil?
    wrong << "#{pass[0]}: #{pass[1]}\n  spinel passed it; ruby never ran it"
  elsif !verdict.empty?
    wrong << "#{pass[0]}: #{pass[1]}\n  spinel passed it; ruby #{verdict}"
  end
end
# Under a `ruby_bug` guard: this Ruby has the bug, so it cannot say.
checked = passes.size - unchecked
note = unchecked.zero? ? "" : " (#{unchecked} more under a `ruby_bug` guard this Ruby skips)"

if wrong.empty?
  puts "#{checked} passing example(s) re-run on #{RUBY_ENGINE} #{RUBY_VERSION}: all agree#{note}"
else
  wrong.each { |w| warn w }
  abort "#{wrong.length} of #{checked} example(s) passed in Spinel but not in Ruby#{note}"
end
