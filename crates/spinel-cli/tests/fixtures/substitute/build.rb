# Build the source `crates/spinel-cli/tests/substitute.rs` serves on
# 127.0.0.1 (#112, docs/package-manager.md, section 6, step 2), and lock
# `app/Gemfile` against it with Bundler:
#
#   ruby crates/spinel-cli/tests/fixtures/substitute/build.rb
#
# `gems/` holds the `.gem`s, made with `Gem::Package.build` from the
# specifications below; `index/` is a compact index of them as rubygems.org
# lays one out: `versions`, its line for each gem ending in the MD5 of the
# gem's `info/<gem>`, and `info/<gem>`, a line per build with its runtime
# dependencies and the SHA-256 of its `.gem`.
#
# - `gizmo 1.0.0`, which `widget` depends on.
# - `widget 1.0.0`, the upstream gem, and `widget-spinel`, Spinel's
#   substitute for it: `1.0.0.1` and `1.0.0.2` (the highest revision, which
#   Spinel picks), a `1.0.0.3` for `java` only (no platform Spinel runs on
#   accepts it), and `1.0.1.1`, which substitutes another upstream version.
#   Each `widget.rb` says which gem it is.
# - `gadget 2.0.0`, which depends on nothing, and `gadget-spinel 2.0.0.1`,
#   which depends on `gizmo`: a substitute that needs what upstream does not.
#
# `app/Gemfile.lock` is what Bundler (4.0.20) writes for `app/Gemfile`
# against this source, its remote put back to rubygems.org's, as the test
# puts its server's in.
#
# Opt-in, never run by CI: it needs Ruby and Bundler. The files it writes are
# committed.

require "digest"
require "fileutils"
require "rubygems/package"
require "socket"
require "tmpdir"

here = __dir__
ENV["SOURCE_DATE_EPOCH"] = "315619200" # 1980-01-02, as RubyGems' default

def spec(name, version, platform: "ruby", deps: {}, impl:)
  spec = Gem::Specification.new do |s|
    s.name = name
    s.version = version
    s.platform = platform
    s.summary = "#{name} #{version}, a fixture for spinel install's substitution"
    s.authors = ["Spinel"]
    s.license = "MIT"
    s.files = ["lib/#{impl[:file]}.rb"]
    s.require_paths = ["lib"]
    deps.each { |dep, req| s.add_runtime_dependency(dep, req) }
  end
  [spec, impl]
end

specs = [
  spec("gizmo", "1.0.0", impl: { file: "gizmo", body: "module Gizmo\n  IMPL = \"gizmo\"\nend\n" }),
  spec("widget", "1.0.0", deps: { "gizmo" => "~> 1.0" },
       impl: { file: "widget", body: "require \"gizmo\"\n\nmodule Widget\n  IMPL = \"widget 1.0.0\"\nend\n" }),
  spec("widget-spinel", "1.0.0.1", deps: { "gizmo" => "~> 1.0" },
       impl: { file: "widget", body: "module Widget\n  IMPL = \"widget-spinel 1.0.0.1\"\nend\n" }),
  spec("widget-spinel", "1.0.0.2", deps: { "gizmo" => "~> 1.0" },
       impl: { file: "widget", body: <<~RUBY }),
         unless RUBY_ENGINE == "spinel"
           raise LoadError, "widget-spinel is Spinel's widget; on \#{RUBY_ENGINE}, install widget"
         end
         require "gizmo"

         module Widget
           IMPL = "widget-spinel 1.0.0.2"
         end
       RUBY
  spec("widget-spinel", "1.0.0.3", platform: "java",
       impl: { file: "widget", body: "module Widget\n  IMPL = \"widget-spinel 1.0.0.3 java\"\nend\n" }),
  spec("widget-spinel", "1.0.1.1",
       impl: { file: "widget", body: "module Widget\n  IMPL = \"widget-spinel 1.0.1.1\"\nend\n" }),
  spec("gadget", "2.0.0", impl: { file: "gadget", body: "module Gadget\n  IMPL = \"gadget 2.0.0\"\nend\n" }),
  spec("gadget-spinel", "2.0.0.1", deps: { "gizmo" => ">= 0" },
       impl: { file: "gadget", body: "module Gadget\n  IMPL = \"gadget-spinel 2.0.0.1\"\nend\n" }),
]

FileUtils.rm_rf(File.join(here, "gems"))
FileUtils.rm_rf(File.join(here, "index"))
FileUtils.mkdir_p(File.join(here, "gems"))
FileUtils.mkdir_p(File.join(here, "index/info"))

built = specs.map do |spec, impl|
  Dir.mktmpdir do |dir|
    Dir.chdir(dir) do
      FileUtils.mkdir_p("lib")
      File.write("lib/#{impl[:file]}.rb", impl[:body])
      file = Gem::Package.build(spec)
      FileUtils.mv(file, File.join(here, "gems", file))
      [spec, File.join(here, "gems", file)]
    end
  end
end

requirement = ->(req) { req.requirements.map { |op, v| "#{op} #{v}" }.sort.join("&") }
versions = +"created_at: 2026-11-18T00:00:00Z\n---\n"
built.group_by { |s, _| s.name }.sort.each do |name, list|
  lines = list.sort_by { |s, _| [s.version, s.platform.to_s] }.map do |spec, path|
    deps = spec.runtime_dependencies.sort_by(&:name).map { |d| "#{d.name}:#{requirement.(d.requirement)}" }
    platform = spec.platform == "ruby" ? "" : "-#{spec.platform}"
    "#{spec.version}#{platform} #{deps.join(",")}|checksum:#{Digest::SHA256.file(path).hexdigest}\n"
  end
  info = "---\n#{lines.join}"
  File.write(File.join(here, "index/info", name), info)
  all = list.sort_by { |s, _| [s.version, s.platform.to_s] }.map do |s, _|
    s.platform == "ruby" ? s.version.to_s : "#{s.version}-#{s.platform}"
  end
  versions << "#{name} #{all.join(",")} #{Digest::MD5.hexdigest(info)}\n"
end
File.write(File.join(here, "index/versions"), versions)

# A server for Bundler: the index and the gems, everything else a 404.
server = TCPServer.new("127.0.0.1", 0)
url = "http://127.0.0.1:#{server.addr[1]}"
Thread.new do
  loop do
    client = server.accept
    request = client.gets.to_s
    nil until (client.gets || "\r\n") == "\r\n"
    path = request.split[1].to_s.sub(%r{\A/}, "")
    file = path.start_with?("gems/") ? File.join(here, path) : File.join(here, "index", path)
    if !path.include?("..") && File.file?(file)
      body = File.binread(file)
      client.write("HTTP/1.1 200 OK\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n", body)
    else
      client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    end
    client.close
  end
end

Dir.mktmpdir do |dir|
  gemfile = File.read(File.join(here, "app/Gemfile"))
  File.write(File.join(dir, "Gemfile"), gemfile.sub("https://rubygems.org", url))
  env = {
    "BUNDLE_GEMFILE" => File.join(dir, "Gemfile"),
    "BUNDLE_USER_HOME" => File.join(dir, "user"),
    "BUNDLE_APP_CONFIG" => File.join(dir, ".bundle"),
  }
  system(env, "bundle", "lock", exception: true)
  lock = File.read(File.join(dir, "Gemfile.lock")).sub("#{url}/", "https://rubygems.org/")
  File.write(File.join(here, "app/Gemfile.lock"), lock)
end
