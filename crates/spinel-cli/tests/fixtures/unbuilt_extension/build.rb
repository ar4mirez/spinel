# Build the source `crates/spinel-cli/tests/unbuilt_extension.rs` serves on
# 127.0.0.1 (#898, docs/package-manager.md, section 6, step 4):
#
#   ruby crates/spinel-cli/tests/fixtures/unbuilt_extension/build.rb
#
# `gems/` holds the `.gem`s, made with `Gem::Package.build` from the
# specifications below; `index/` is a compact index of them as rubygems.org
# lays one out (as `../substitute/build.rb` makes one).
#
# - `plain 1.0.0`, pure Ruby.
# - `cog 1.0.0` and `ratchet 1.0.0`, each with a C extension
#   (`ext/<gem>/extconf.rb`) and no substitute.
# - `sprocket 1.0.0`, with a C extension, and `sprocket-spinel 1.0.0.1`,
#   Spinel's substitute for it, pure Ruby.
#
# Opt-in, never run by CI: it needs Ruby. The files it writes are committed.

require "digest"
require "fileutils"
require "rubygems/package"
require "tmpdir"

here = __dir__
ENV["SOURCE_DATE_EPOCH"] = "315619200" # 1980-01-02, as RubyGems' default

def spec(name, version, ext: false, body:)
  file = name.delete_suffix("-spinel")
  files = { "lib/#{file}.rb" => body }
  if ext
    files["ext/#{file}/extconf.rb"] = "require \"mkmf\"\ncreate_makefile(\"#{file}/#{file}\")\n"
    files["ext/#{file}/#{file}.c"] = "#include <ruby.h>\n\nvoid Init_#{file}(void) {}\n"
  end
  spec = Gem::Specification.new do |s|
    s.name = name
    s.version = version
    s.summary = "#{name} #{version}, a fixture for spinel install's unbuilt extensions"
    s.authors = ["Spinel"]
    s.license = "MIT"
    s.files = files.keys.sort
    s.require_paths = ["lib"]
    s.extensions = ["ext/#{file}/extconf.rb"] if ext
  end
  [spec, files]
end

specs = [
  spec("plain", "1.0.0", body: "module Plain\n  IMPL = \"plain 1.0.0\"\nend\n"),
  spec("cog", "1.0.0", ext: true, body: "require \"cog/cog\"\n"),
  spec("ratchet", "1.0.0", ext: true, body: "require \"ratchet/ratchet\"\n"),
  spec("sprocket", "1.0.0", ext: true, body: "require \"sprocket/sprocket\"\n"),
  spec("sprocket-spinel", "1.0.0.1",
       body: "module Sprocket\n  IMPL = \"sprocket-spinel 1.0.0.1\"\nend\n"),
]

FileUtils.rm_rf(File.join(here, "gems"))
FileUtils.rm_rf(File.join(here, "index"))
FileUtils.mkdir_p(File.join(here, "gems"))
FileUtils.mkdir_p(File.join(here, "index/info"))

built = specs.map do |spec, files|
  Dir.mktmpdir do |dir|
    Dir.chdir(dir) do
      files.each do |path, text|
        FileUtils.mkdir_p(File.dirname(path))
        File.write(path, text)
      end
      file = Gem::Package.build(spec)
      FileUtils.mv(file, File.join(here, "gems", file))
      [spec, File.join(here, "gems", file)]
    end
  end
end

versions = +"created_at: 2026-11-25T00:00:00Z\n---\n"
built.group_by { |s, _| s.name }.sort.each do |name, list|
  lines = list.sort_by { |s, _| s.version }.map do |spec, path|
    "#{spec.version} |checksum:#{Digest::SHA256.file(path).hexdigest}\n"
  end
  info = "---\n#{lines.join}"
  File.write(File.join(here, "index/info", name), info)
  all = list.sort_by { |s, _| s.version }.map { |s, _| s.version.to_s }
  versions << "#{name} #{all.join(",")} #{Digest::MD5.hexdigest(info)}\n"
end
File.write(File.join(here, "index/versions"), versions)
