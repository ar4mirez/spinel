# Take what `crates/spinel-cli/tests/nio4r.rs` serves on 127.0.0.1 in place
# of rubygems.org (#115), lock `app/Gemfile` against it with Bundler, and
# record what CRuby's nio4r answers `surface.rb`:
#
#   ruby crates/spinel-cli/tests/fixtures/nio4r/build.rb
#
# - `index/` is a compact index of rubygems.org's own `/info` lines for the
#   versions locked below, the `created_at:` requirement dropped as
#   `../resolver/snapshot.sh` drops it. `versions` lists them as rubygems.org
#   lays it out, each line ending in the MD5 of its `info/` file.
#   nio4r-spinel is not in it: the test builds that gem and adds its line.
# - `../install/gems/` gets rubygems.org's `.gem` of each locked gem but
#   nio4r, which is never fetched: RSpec's, which run nio4r's suite.
# - `app/Gemfile.lock` is what Bundler (4.0.20) locks for `app/Gemfile`
#   against that index, for x86_64-linux, aarch64-linux, arm64-darwin and
#   x86_64-darwin, its remote put back to rubygems.org's, as the test puts
#   its server's in.
# - `surface.txt` is `surface.rb`'s output on this Ruby with nio4r 2.7.5
#   installed from rubygems.org into a scratch directory, its C extension
#   compiled, on Linux.
#
# Opt-in, never run by CI: it needs the network, Ruby, Bundler and a C
# compiler. The files it writes are committed. `app/spec/` is not its:
# `gems/nio4r-spinel/vendor.sh` vendors it.

require "digest"
require "fileutils"
require "net/http"
require "socket"
require "tmpdir"

here = __dir__
locked = {
  "diff-lcs" => "1.6.2",
  "nio4r" => "2.7.5",
  "rspec-core" => "3.13.7",
  "rspec-expectations" => "3.13.6",
  "rspec-mocks" => "3.13.9",
  "rspec-support" => "3.13.8"
}

FileUtils.rm_rf(File.join(here, "index"))
FileUtils.mkdir_p(File.join(here, "index/info"))
versions = +"created_at: 2026-11-18T00:00:00Z\n---\n"
locked.sort.each do |name, version|
  body = Net::HTTP.get(URI("https://rubygems.org/info/#{name}"))
  lines = body.lines.select { |line| line.start_with?("#{version} ", "#{version}-") }
  abort "build.rb: rubygems.org has no #{name} #{version}" if lines.empty?
  lines = lines.map { |line| line.sub(/,created_at:[^,|\n]*/, "").sub(/\|created_at:[^,\n]*,?/, "|") }
  info = "---\n#{lines.join}"
  File.write(File.join(here, "index/info", name), info)
  builds = lines.map { |line| line.split(" ", 2).first }
  versions << "#{name} #{builds.join(",")} #{Digest::MD5.hexdigest(info)}\n"
  next if name == "nio4r"

  gem = "#{name}-#{version}.gem"
  bytes = Net::HTTP.get(URI("https://rubygems.org/gems/#{gem}"))
  checksum = lines.first[/checksum:(\h+)/, 1]
  abort "build.rb: #{gem} is not the gem its checksum names" unless Digest::SHA256.hexdigest(bytes) == checksum
  File.binwrite(File.join(here, "../install/gems", gem), bytes)
end
File.write(File.join(here, "index/versions"), versions)

# A server for Bundler: the index, everything else a 404.
server = TCPServer.new("127.0.0.1", 0)
source = "http://127.0.0.1:#{server.addr[1]}"
Thread.new do
  loop do
    client = server.accept
    request = client.gets.to_s
    nil until (client.gets || "\r\n") == "\r\n"
    path = request.split[1].to_s.sub(%r{\A/}, "")
    file = File.join(here, "index", path)
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
  File.write(File.join(dir, "Gemfile"), gemfile.sub("https://rubygems.org", source))
  env = {
    "BUNDLE_GEMFILE" => File.join(dir, "Gemfile"),
    "BUNDLE_USER_HOME" => File.join(dir, "user"),
    "BUNDLE_APP_CONFIG" => File.join(dir, ".bundle")
  }
  system(env, "bundle", "lock", "--add-platform", "x86_64-linux", "aarch64-linux", "arm64-darwin", "x86_64-darwin",
    exception: true)
  lock = File.read(File.join(dir, "Gemfile.lock")).sub("#{source}/", "https://rubygems.org/")
  File.write(File.join(here, "app/Gemfile.lock"), lock)
end

Dir.mktmpdir do |dir|
  system("gem", "install", "nio4r", "--version", locked.fetch("nio4r"), "--install-dir", dir,
    "--no-document", exception: true)
  env = {"GEM_HOME" => dir, "GEM_PATH" => dir}
  output = IO.popen(env, ["ruby", File.join(here, "surface.rb")], err: [:child, :out], &:read)
  File.write(File.join(here, "surface.txt"), output)
end
