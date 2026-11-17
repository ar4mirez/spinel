# Build the two indexes `crates/spinel-cli/tests/private_source.rs` serves on
# 127.0.0.1 (#89), from the real `.gem` files `../install/gems/` holds:
#
#   ruby crates/spinel-cli/tests/fixtures/private_source/snapshot.rb
#
# `compact/` is a compact index as rubygems.org lays it out: `versions`, its
# line for each gem ending in the MD5 of the gem's `info/<gem>`, and
# `info/<gem>`, a line per version with its runtime dependencies and the
# SHA-256 of its `.gem`. `legacy/` is the index a server without one has, as
# `gem generate_index` writes it: `specs.4.8.gz` and
# `prerelease_specs.4.8.gz` (a gzipped Marshal of `[name, Gem::Version,
# platform]` triples) and `quick/Marshal.4.8/<full name>.gemspec.rz` (a
# deflated Marshal of each gem's specification). Both list the same gems:
# rack-session and rackup and what they depend on, one version each.
#
# Opt-in, never run by CI: it needs Ruby. The files it writes are committed.

require "digest"
require "fileutils"
require "rubygems/package"
require "zlib"

here = __dir__
gems = File.expand_path("../install/gems", here)
names = %w[base64-0.3.0 rack-3.2.8 rack-session-2.1.2 rackup-2.3.1]
specs = names.map { |n| [Gem::Package.new(File.join(gems, "#{n}.gem")).spec, File.join(gems, "#{n}.gem")] }

FileUtils.rm_rf(File.join(here, "compact"))
FileUtils.rm_rf(File.join(here, "legacy"))
FileUtils.mkdir_p(File.join(here, "compact/info"))
FileUtils.mkdir_p(File.join(here, "legacy/quick/Marshal.4.8"))

requirement = ->(req) { req.requirements.map { |op, v| "#{op} #{v}" }.sort.join("&") }
versions = +"created_at: 2026-11-16T00:00:00Z\n---\n"
specs.sort_by { |s, _| s.name }.each do |spec, path|
  deps = spec.runtime_dependencies.sort_by(&:name).map { |d| "#{d.name}:#{requirement.(d.requirement)}" }
  reqs = ["checksum:#{Digest::SHA256.file(path).hexdigest}"]
  reqs << "ruby:#{requirement.(spec.required_ruby_version)}" unless spec.required_ruby_version.none?
  reqs << "rubygems:#{requirement.(spec.required_rubygems_version)}" unless spec.required_rubygems_version.none?
  info = "---\n#{spec.version} #{deps.join(",")}|#{reqs.join(",")}\n"
  File.write(File.join(here, "compact/info", spec.name), info)
  versions << "#{spec.name} #{spec.version} #{Digest::MD5.hexdigest(info)}\n"

  rz = Zlib::Deflate.deflate(Marshal.dump(spec))
  File.binwrite(File.join(here, "legacy/quick/Marshal.4.8", "#{spec.full_name}.gemspec.rz"), rz)
end
File.write(File.join(here, "compact/versions"), versions)

tuples = specs.map { |s, _| [s.name, s.version, s.original_platform.to_s] }
release, pre = tuples.partition { |_, v, _| !v.prerelease? }
{ "specs" => release, "prerelease_specs" => pre }.each do |name, list|
  Zlib::GzipWriter.open(File.join(here, "legacy/#{name}.4.8.gz")) do |gz|
    gz.mtime = 0
    gz.write(Marshal.dump(list))
  end
end
