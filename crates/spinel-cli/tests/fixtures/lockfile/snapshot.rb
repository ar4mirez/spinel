# Take the lockfile fixtures' compact index snapshot (#80) from rubygems.org:
#
#   ruby crates/spinel-cli/tests/fixtures/lockfile/snapshot.rb
#   crates/spinel-cli/tests/fixtures/lockfile/bundler.sh
#
# `index/info/<gem>` is rubygems.org's `/info/<gem>` for every gem the roots
# below reach, trimmed so a Rails-sized dependency tree stays a small
# fixture: of each gem only the builds (every platform's) of its three
# highest versions are kept, and of the highest version that meets each
# requirement a kept build puts on it, and the tree is walked through those
# alone. Each kept line is as rubygems.org serves it but for its `created_at:`, which
# only Bundler's cooldown reads. `index/versions` is made from the info
# files as rubygems.org lays `/versions` out, each gem's line ending in the
# MD5 of its info file. `index/SOURCE` records when it was taken. `bundler`
# is left out: Bundler resolves a dependency on it to itself, never to a
# source, and so does Spinel.
#
# Taking it again moves every gem to what rubygems.org has now, so run
# `bundler.sh` after it: that rewrites every case's `Gemfile.lock`.
#
# Written with Ruby 4.0.9 (RubyGems 4.0.20).

require "digest/md5"
require "fileutils"
require "net/http"
require "time"

ROOTS = %w[
  rails propshaft sqlite3 puma importmap-rails turbo-rails stimulus-rails
  jbuilder tzinfo-data solid_cache solid_queue solid_cable bootsnap kamal
  thruster debug bundler-audit brakeman rubocop-rails-omakase web-console
  capybara selenium-webdriver nokogiri json rack
].freeze
KEEP = 3

here = __dir__
index = File.join(here, "index")
FileUtils.rm_rf(index)
FileUtils.mkdir_p(File.join(index, "info"))

def fetch(gem)
  uri = URI("https://rubygems.org/info/#{gem}")
  response = Net::HTTP.get_response(uri)
  raise "#{uri}: #{response.code}" unless response.is_a?(Net::HTTPSuccess)
  response.body
end

# Every gem's lines, fetched once.
LINES = Hash.new do |h, gem|
  h[gem] = fetch(gem).lines.map(&:chomp).drop_while {|l| l != "---" }.drop(1).reject(&:empty?)
end

def version_of(line) = line.split(" ", 2).first.split("-", 2).first

# `[name, requirement]` for each dependency of a line.
def dependencies_of(line)
  line.split(" ", 2)[1].to_s.split("|", 2).first.to_s.split(",").map do |dep|
    name, reqs = dep.split(":", 2)
    [name, Gem::Requirement.new(reqs.to_s.split("&"))]
  end
end

# The versions kept of each gem reached: its three highest, and the highest
# that meets each requirement a kept build puts on it, so a version some kept
# build needs is never trimmed away. Worked to a fixed point.
kept = {}
loop do
  wanted = Hash.new {|h, k| h[k] = [] }
  ROOTS.each {|gem| wanted[gem] }
  kept.each do |gem, versions|
    LINES[gem].select {|l| versions.include?(version_of(l)) }.each do |line|
      dependencies_of(line).each {|name, req| wanted[name] << req unless name == "bundler" }
    end
  end
  now = wanted.to_h do |gem, reqs|
    versions = LINES[gem].map {|l| version_of(l) }.uniq.sort_by {|v| Gem::Version.new(v) }
    chosen = versions.last(KEEP)
    reqs.each do |req|
      best = versions.reverse.find {|v| req.satisfied_by?(Gem::Version.new(v)) }
      chosen |= [best] if best
    end
    [gem, chosen]
  end
  break if now == kept
  kept = now
end

kept.each do |gem, versions|
  lines = LINES[gem].select {|l| versions.include?(version_of(l)) }.map do |line|
    line.sub(/,created_at:[^,|]*/, "").sub(/\|created_at:[^,]*,?/, "|")
  end
  File.write(File.join(index, "info", gem), "---\n" + lines.map {|l| "#{l}\n" }.join)
end

taken = Time.now.utc.iso8601
File.open(File.join(index, "versions"), "w") do |out|
  out.puts "created_at: #{taken}"
  out.puts "---"
  Dir[File.join(index, "info", "*")].sort.each do |info|
    body = File.read(info)
    versions = body.lines.drop(1).map {|l| l.split(" ", 2).first }.join(",")
    out.puts "#{File.basename(info)} #{versions} #{Digest::MD5.hexdigest(body)}"
  end
end
File.write(File.join(index, "SOURCE"),
  "rubygems.org compact index, /info of #{ROOTS.join(" ")} and what they reach, " \
  "trimmed to the versions they need and the three highest of each, taken #{taken}\n")
