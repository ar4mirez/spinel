# Build the pg-spinel gem for this machine:
#
#   spinel run gems/pg-spinel/build.rb [options]
#   ruby gems/pg-spinel/build.rb [options]
#
# writes `pg-spinel-1.7.0.1-<platform>.gem`: cargo builds the extension
# (`ext/pg_spinel`, a workspace member, linked with the system's libpq,
# which `pg_config` or `$PQ_LIB_DIR` names), and a copy of this directory
# with the library at `lib/pg_ext/pg_spinel.spinel` is packaged by
# RubyGems' own `Gem::Package.build`, from `pg-spinel.gemspec` with the
# platform and the library added.
#
# The platform is the one docs/package-manager.md (section 6, step 2) gives
# for the machine, as Spinel's `Gem::Platform.local` names it with Linux's
# libc said: `x86_64-linux-gnu`, `aarch64-linux-musl`, `arm64-darwin`.
#
# Options:
#
#   --out DIR         where the .gem goes (default: the current directory)
#   --profile NAME    cargo's profile (default: release)
#   --target-dir DIR  cargo's target directory (default: cargo's own)
#   --offline         build with cargo --offline
#   --jobs N          cargo --jobs N
#
# Nothing is published. Publishing to rubygems.org waits until the project
# owns the name (docs/roadmap.md).

require "fileutils"
require "optparse"
require "rubygems/package"
require "tmpdir"

here = __dir__
root = File.expand_path("../..", here)
options = {out: Dir.pwd, profile: "release", target_dir: nil, offline: false, jobs: nil}
OptionParser.new do |o|
  o.on("--out DIR") { |v| options[:out] = File.expand_path(v) }
  o.on("--profile NAME") { |v| options[:profile] = v }
  o.on("--target-dir DIR") { |v| options[:target_dir] = File.expand_path(v) }
  o.on("--offline") { options[:offline] = true }
  o.on("--jobs N", Integer) { |v| options[:jobs] = v }
end.parse!

# The platform this machine's builds are published under.
def spinel_platform
  local = Gem::Platform.local
  case local.os
  when "linux" then Gem::Platform.new([local.cpu, "linux", local.version || "gnu"])
  when "darwin" then Gem::Platform.new([local.cpu, "darwin", nil])
  else local
  end
end

cargo = ENV.fetch("CARGO", "cargo")
command = [cargo, "build", "--quiet", "--package", "pg-spinel", "--profile", options[:profile],
  "--manifest-path", File.join(root, "Cargo.toml")]
command += ["--target-dir", options[:target_dir]] if options[:target_dir]
command << "--offline" if options[:offline]
command += ["--jobs", options[:jobs].to_s] if options[:jobs]
system(*command, chdir: root) or abort "build.rb: #{command.join(" ")} failed"

target = options[:target_dir] || File.join(root, "target")
profile_dir = options[:profile] == "dev" ? "debug" : options[:profile]
library = %w[libpg_spinel.so libpg_spinel.dylib]
  .map { |name| File.join(target, profile_dir, name) }
  .find { |path| File.exist?(path) }
abort "build.rb: cargo wrote no library in #{File.join(target, profile_dir)}" unless library

gem = Dir.mktmpdir("pg-spinel") do |stage|
  FileUtils.cp_r(File.join(here, "lib"), stage)
  FileUtils.cp(File.join(here, "LICENSE-pg"), stage)
  FileUtils.cp(File.join(here, "BSDL-pg"), stage)
  FileUtils.cp(library, File.join(stage, "lib/pg_ext/pg_spinel.spinel"))
  spec = Gem::Specification.load(File.join(here, "pg-spinel.gemspec"))
  spec.platform = spinel_platform
  spec.files += ["lib/pg_ext/pg_spinel.spinel"]
  Dir.chdir(stage) do
    file = Gem::Package.build(spec)
    FileUtils.mkdir_p(options[:out])
    FileUtils.mv(file, File.join(options[:out], file))
    File.join(options[:out], file)
  end
end
puts gem
