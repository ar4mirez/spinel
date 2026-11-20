# frozen_string_literal: true

# The oracle behind `solver_oracle.txt`, which the tests of
# `crates/spinel-cli/src/resolver/solver/` (the port of Bundler's vendored
# pubgrub-ruby, #816) compare against. It runs Bundler 4.0.20's
# `bundler/vendor/pub_grub` (0.5.0) on CRuby 4.0.9 and writes:
#
# - `== ranges`: set operations on ranges made from requirements, as
#   `PubGrub::RubyGems` makes them over `Gem::Version`s;
# - `== candidates`: ranges as `Bundler::Resolver#requirement_to_range`
#   makes them over the two candidates of each version, and the candidates
#   each selects;
# - `== example <name>`: each worked example of Dart pub's `doc/solver.md`,
#   and three more, through `StaticPackageSource`, `^x.y.z` written `~> x.y` as
#   pubgrub-ruby's own tests write it: the solver's debug log, then the
#   solution, or the failure and its derivation (each incompatibility, then
#   indented, the two it was derived from), then `FailureWriter`'s
#   explanation of it.
#
#   ruby solver_oracle.rb            # rewrite solver_oracle.txt
#   ruby solver_oracle.rb --check    # agree with it, or fail
#
# An opt-in tool, not run by CI: the Rust tests read the table it wrote.

require "bundler"
require "bundler/vendor/pub_grub/lib/pub_grub"
require "logger"

abort "Bundler 4.0.20 expected, not #{Bundler::VERSION}" unless Bundler::VERSION == "4.0.20"
abort "pub_grub 0.5.0 expected" unless Bundler::PubGrub::VERSION == "0.5.0"

TABLE = File.join(__dir__, "solver_oracle.txt")
PG = Bundler::PubGrub

# A range written as requirements, `|` between the ones unioned and `, `
# between the pairs of one requirement.
SETS = [
  "~> 1.0", ">= 1.0", "> 1.0", "< 2.0", "<= 2.0", "= 1.5", "!= 1.5",
  ">= 1.0, < 2.0", "~> 1.5.1", ">= 0", "!= 1.0, != 2.0",
  "~> 2.0 | ~> 1.0", "< 1.0 | >= 2.0", "= 1.0 | = 1.5 | = 2.0",
  "< 1.0 | > 1.5, < 2.0 | >= 3.0",
].freeze

VERSIONS = %w[0.9 1.0 1.0.1 1.5 1.5.1 1.6 2.0.a 2.0 2.1 3.0].freeze

def set(text)
  text.split(" | ").map { |req| PG::RubyGems.parse_range(req.split(", ")) }.inject(:union)
end

def ranges_section(out)
  versions = VERSIONS.map { |v| Gem::Version.new(v) }.sort
  out << "== ranges"
  SETS.each do |a|
    ra = set(a)
    out << "to_s | #{a} | | #{ra}"
    out << "invert | #{a} | | #{ra.invert}"
    out << "upper_invert | #{a} | | #{ra.upper_invert}"
    out << "select | #{a} | | #{ra.select_versions(versions).join(" ")}"
    SETS.each do |b|
      rb = set(b)
      out << "intersect | #{a} | #{b} | #{ra.intersect(rb)}"
      out << "union | #{a} | #{b} | #{ra.union(rb)}"
      out << "allows_all | #{a} | #{b} | #{ra.allows_all?(rb)}"
      out << "intersects | #{a} | #{b} | #{!!ra.intersects?(rb)}"
    end
  end
end

def candidates_section(out)
  resolver = Bundler::Resolver.allocate
  candidates = VERSIONS.flat_map do |v|
    [-1, 1].map { |p| Bundler::Resolver::Candidate.new(v, priority: p) }
  end.sort
  label = ->(c) { "#{c.version}/#{c.send(:priority)}" }
  out << "== candidates"
  SETS.each do |text|
    range = text.split(" | ").map do |req|
      resolver.send(:requirement_to_range, Gem::Requirement.new(req.split(", ")))
    end.inject(:union)
    out << "to_s | #{text} | | #{range}"
    out << "select | #{text} | | #{range.select_versions(candidates).map(&label).join(" ")}"
  end
end

EXAMPLES = {
  "no_conflicts" => ->(s) {
    s.root deps: { "foo" => "~> 1.0" }
    s.add "foo", "1.0.0", deps: { "bar" => "~> 1.0" }
    s.add "bar", "1.0.0"
    s.add "bar", "2.0.0"
  },
  "avoiding_conflict_during_decision_making" => ->(s) {
    s.root deps: { "foo" => "~> 1.0", "bar" => "~> 1.0" }
    s.add "foo", "1.1.0", deps: { "bar" => "~> 2.0" }
    s.add "foo", "1.0.0"
    s.add "bar", "1.0.0"
    s.add "bar", "1.1.0"
    s.add "bar", "2.0.0"
  },
  "performing_conflict_resolution" => ->(s) {
    s.root deps: { "foo" => ">= 1.0.0" }
    s.add "foo", "2.0.0", deps: { "bar" => "~> 1.0" }
    s.add "foo", "1.0.0"
    s.add "bar", "1.0.0", deps: { "foo" => "~> 1.0" }
  },
  "conflict_resolution_with_a_partial_satisfier" => ->(s) {
    s.root deps: { "foo" => "~> 1.0", "target" => "~> 2.0" }
    s.add "foo", "1.1.0", deps: { "left" => "~> 1.0", "right" => "~> 1.0" }
    s.add "foo", "1.0.0"
    s.add "left", "1.0.0", deps: { "shared" => ">= 1.0.0" }
    s.add "right", "1.0.0", deps: { "shared" => "< 2.0.0" }
    s.add "shared", "2.0.0"
    s.add "shared", "1.0.0", deps: { "target" => "~> 1.0" }
    s.add "target", "2.0.0"
    s.add "target", "1.0.0"
  },
  "linear_error_reporting" => ->(s) {
    s.root deps: { "foo" => "~> 1.0", "baz" => "~> 1.0" }
    s.add "foo", "1.0.0", deps: { "bar" => "~> 2.0" }
    s.add "bar", "2.0.0", deps: { "baz" => "~> 3.0" }
    s.add "baz", "1.0.0"
    s.add "baz", "3.0.0"
  },
  "branching_error_reporting" => ->(s) {
    s.root deps: { "foo" => "~> 1.0" }
    s.add "foo", "1.0.0", deps: { "a" => "~> 1.0", "b" => "~> 1.0" }
    s.add "foo", "1.1.0", deps: { "x" => "~> 1.0", "y" => "~> 1.0" }
    s.add "a", "1.0.0", deps: { "b" => "~> 2.0" }
    s.add "b", "1.0.0"
    s.add "b", "2.0.0"
    s.add "x", "1.0.0", deps: { "y" => "~> 2.0" }
    s.add "y", "1.0.0"
    s.add "y", "2.0.0"
  },
  # Beyond solver.md: a dependency on a package the source does not have,
  # a requirement no version meets, and a version tried after a backtrack
  # whose dependency the solver already knows.
  "unknown_package" => ->(s) {
    s.root deps: { "foo" => ">= 1.0" }
    s.add "foo", "2.0.0", deps: { "missing" => ">= 1.0" }
    s.add "foo", "1.0.0"
  },
  "no_versions" => ->(s) {
    s.root deps: { "foo" => "~> 1.0" }
    s.add "foo", "1.0.0", deps: { "bar" => "~> 2.0" }
    s.add "bar", "1.0.0"
  },
  "knew_incompatibility" => ->(s) {
    s.root deps: { "foo" => ">= 1.0", "baz" => "~> 1.0" }
    s.add "foo", "1.1.0", deps: { "bar" => "~> 1.0", "qux" => "~> 1.0" }
    s.add "foo", "1.0.0", deps: { "bar" => "~> 1.0" }
    s.add "qux", "1.0.0", deps: { "baz" => "~> 2.0" }
    s.add "bar", "1.0.0", deps: { "bar" => ">= 1.0" }
    s.add "baz", "1.0.0"
    s.add "baz", "2.0.0"
  },
}.freeze

LOG = []

class Collect < Logger
  def initialize
    super(nil)
    self.level = Logger::DEBUG
  end

  def add(severity, message = nil, progname = nil)
    return true if severity < level

    message = yield if message.nil? && block_given?
    LOG << message.to_s
    true
  end
end

PG.logger = Collect.new

def derivation(incompatibility, depth, out)
  out << "#{"  " * depth}#{incompatibility}"
  return unless incompatibility.conflict?

  derivation(incompatibility.cause.incompatibility, depth + 1, out)
  derivation(incompatibility.cause.satisfier, depth + 1, out)
end

def examples_section(out)
  EXAMPLES.each do |name, body|
    LOG.clear
    source = PG::StaticPackageSource.new { |s| body.(s) }
    result = []
    begin
      solution = PG::VersionSolver.new(source: source).solve
      result << "solution:"
      solution.each { |package, version| result << "#{package} #{version}" }
    rescue PG::SolveFailure => e
      result << "failure:"
      derivation(e.incompatibility, 0, result)
      result << "-- explanation"
      result.concat(e.explanation.split("\n", -1))
    end
    out << "== example #{name}"
    out << "-- log"
    out.concat(LOG)
    out << "-- result"
    out.concat(result)
  end
end

out = []
ranges_section(out)
candidates_section(out)
examples_section(out)
text = out.join("\n") + "\n"

if ARGV.include?("--check")
  abort "#{TABLE} differs from the oracle; rerun without --check" unless File.read(TABLE) == text
  puts "agrees"
else
  File.write(TABLE, text)
end
