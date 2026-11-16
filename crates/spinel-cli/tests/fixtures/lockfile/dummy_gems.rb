# Build a stand-in `.gem` for every spec a lockfile locks, for
# `install_check.sh` (#80):
#
#   ruby dummy_gems.rb <Gemfile.lock> <index dir> <out dir>
#
# Each has the spec's name, version, platform and runtime dependencies, and
# the `ruby:` and `rubygems:` requirements of its line in `<index
# dir>/info/<gem>`, so Bundler installing it finds what the index and the
# lockfile say; it has no files. Then `<index dir>` is rewritten in place:
# each built spec's `checksum:` becomes the SHA-256 of its stand-in, and
# `versions` is made again from the info files, so Bundler verifies what it
# downloads against the index it resolved with.

require "bundler"
require "digest"
require "fileutils"
require "rubygems/package"

lockfile, index, out = ARGV
abort "usage: dummy_gems.rb <Gemfile.lock> <index dir> <out dir>" unless out

FileUtils.mkdir_p(out)
Gem::DefaultUserInteraction.ui = Gem::SilentUI.new
parser = Bundler::LockfileParser.new(File.read(lockfile))
checksums = {}
parser.specs.each do |locked|
  next unless locked.source.is_a?(Bundler::Source::Rubygems)

  platform = locked.platform.to_s
  build = platform == "ruby" ? locked.version.to_s : "#{locked.version}-#{platform}"
  line = File.readlines(File.join(index, "info", locked.name), chomp: true).find {|l| l.split(" ", 2).first == build }
  abort "#{locked.full_name}: not in #{index}" unless line
  fields = line.split("|", 2)[1].to_s.split(",").to_h {|f| f.split(":", 2) }

  spec = Gem::Specification.new do |s|
    s.name = locked.name
    s.version = locked.version
    s.platform = locked.platform
    s.summary = "A stand-in for #{locked.full_name}"
    s.authors = ["Spinel"]
    s.files = []
    s.required_ruby_version = fields["ruby"].split("&") if fields["ruby"]
    s.required_rubygems_version = fields["rubygems"].split("&") if fields["rubygems"]
    locked.dependencies.each {|dep| s.add_dependency(dep.name, *dep.requirement.as_list) }
  end
  path = Dir.chdir(out) { File.expand_path(Gem::Package.build(spec)) }
  checksums[[locked.name, build]] = Digest::SHA256.file(path).hexdigest
end

Dir[File.join(index, "info", "*")].each do |info|
  name = File.basename(info)
  lines = File.readlines(info, chomp: true).map do |l|
    sum = checksums[[name, l.split(" ", 2).first]]
    sum ? l.sub(/checksum:[0-9a-f]+/, "checksum:#{sum}") : l
  end
  File.write(info, lines.map {|l| "#{l}\n" }.join)
end
File.open(File.join(index, "versions"), "w") do |f|
  f.puts "created_at: 2026-01-01T00:00:00Z"
  f.puts "---"
  Dir[File.join(index, "info", "*")].sort.each do |info|
    body = File.read(info)
    versions = body.lines.drop(1).map {|l| l.split(" ", 2).first }.join(",")
    f.puts "#{File.basename(info)} #{versions} #{Digest::MD5.hexdigest(body)}"
  end
end
puts "#{checksums.size} stand-in gems"
