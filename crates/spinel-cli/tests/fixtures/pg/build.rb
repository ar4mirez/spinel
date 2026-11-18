# Take what `crates/spinel-cli/tests/pg.rs` serves on 127.0.0.1 in place of
# rubygems.org (#114), lock `app/Gemfile` against it with Bundler, and
# record what CRuby's pg answers `surface.rb`:
#
#   SPINEL_PG_TEST_URL=postgres://... ruby crates/spinel-cli/tests/fixtures/pg/build.rb
#
# - `index/` is a compact index of rubygems.org's own `/info` line for pg
#   1.7.0, every build of it, the `created_at:` requirement dropped as
#   `../resolver/snapshot.sh` drops it. `versions` lists it as rubygems.org
#   lays it out, its line ending in the MD5 of `info/pg`. pg-spinel is not in
#   it: the test builds that gem and adds its line.
# - `app/Gemfile.lock` is what Bundler (4.0.20) locks for `app/Gemfile`
#   against that index, for x86_64-linux, aarch64-linux, arm64-darwin and
#   x86_64-darwin, its remote put back to rubygems.org's, as the test puts
#   its server's in.
# - `surface.txt` is `surface.rb`'s output on this Ruby with pg 1.7.0
#   installed from rubygems.org into a scratch directory and built from its
#   source against the system's libpq (`--platform ruby`), as pg-spinel
#   links it, run against the server `SPINEL_PG_TEST_URL` names.
#
# Opt-in, never run by CI: it needs the network, Ruby, Bundler, libpq's
# headers and a PostgreSQL server. The files it writes are committed.

require "digest"
require "fileutils"
require "net/http"
require "socket"
require "tmpdir"

here = __dir__
url = ENV.fetch("SPINEL_PG_TEST_URL") { abort "build.rb: set SPINEL_PG_TEST_URL to a PostgreSQL server's URL" }
locked = {"pg" => "1.7.0"}

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
  system("gem", "install", "pg", "--version", "1.7.0", "--platform", "ruby", "--install-dir", dir,
    "--no-document", exception: true)
  env = {"GEM_HOME" => dir, "GEM_PATH" => dir}
  output = IO.popen(env, ["ruby", File.join(here, "surface.rb")], err: [:child, :out], &:read)
  File.write(File.join(here, "surface.txt"), output)
end
