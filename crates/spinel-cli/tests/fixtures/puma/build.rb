# Take what `crates/spinel-cli/tests/puma.rs` serves on 127.0.0.1 in place of
# rubygems.org (#116), lock `app/Gemfile` and `sinatra/Gemfile` against it
# with Bundler, and record what CRuby with upstream Puma 8.1.2 answers:
#
#   ruby crates/spinel-cli/tests/fixtures/puma/build.rb [--suite]
#
# - `index/` is a compact index of rubygems.org's own `/info` lines for the
#   versions the two lockfiles lock, the `created_at:` requirement dropped as
#   `../resolver/snapshot.sh` drops it. `versions` lists them as rubygems.org
#   lays it out, each line ending in the MD5 of its `info/` file. The
#   `-spinel` gems are not in it: the test builds those and adds their lines.
# - `../install/gems/` gets rubygems.org's `.gem` of each locked gem the test
#   installs: not nio4r and puma, which are substituted and never fetched, nor
#   bigdecimal and json, which are Spinel's built-ins.
# - `app/Gemfile.lock` and `sinatra/Gemfile.lock` are what Bundler (4.0.20)
#   locks for each Gemfile against that index, for x86_64-linux,
#   aarch64-linux, arm64-darwin and x86_64-darwin, their remote put back to
#   rubygems.org's, as the test puts its server's in. The Sinatra app's starts
#   from `../sinatra-lock`'s lockfile, so it keeps that fixture's versions and
#   adds Puma in WEBrick's place.
# - `surface.txt` is `surface.rb`'s output, and `sinatra.txt` the response
#   `sinatra/app.rb` serves on Puma, on this Ruby with the locked gems
#   installed from rubygems.org into a scratch directory, Puma's C extension
#   compiled, on Linux.
# - With `--suite`, `suite.txt`: each test of Puma's own suite (`app/test/`,
#   which `gems/puma-spinel/vendor.sh` vendors) and how it ended on this Ruby
#   with upstream Puma, run as `tests/puma.rs` runs it on Spinel, offline: in
#   a network namespace of its own (`unshare -rn`), with only loopback up,
#   and `TestPumaServerSSLKeyUpdate` flooding the server with KeyUpdates from
#   the client cargo builds of
#   `gems/puma-spinel/ext/puma_spinel/examples/key_update_peer.rs`
#   (`key_update_peer.rb`) in place of the C one it compiles, as on Spinel.
#
# Opt-in, never run by CI: it needs the network, Ruby 4.0.9, Bundler, cargo
# and a C compiler. The files it writes are committed.

require "digest"
require "fileutils"
require "net/http"
require "socket"
require "tmpdir"

here = __dir__
gems_dir = File.join(here, "../install/gems")
suite = ARGV.include?("--suite")

# What each Gemfile locks: read back from the lockfiles this writes, so a
# re-run after a Gemfile edit starts from what was locked.
def locked_specs(lock)
  lock[/^GEM\n.*?\n\n/m].to_s.scan(/^    (\S+) \(([^)]+)\)$/).to_h
end

substituted = %w[nio4r puma]
# What neither run of the suite runs, as `tests/puma.rs` leaves it out (its
# `EXCLUDED` says why, test by test): what needs rubygems.org, or the
# interpreter to be MRI.
EXCLUDED = "/\\A(?:TestWorkerGemIndependence|TestIntegrationSSLSession)#" \
  "|\\A(?:TestPluginSystemd#test_systemd_(?:cluster_)?notify" \
  "|TestIntegrationPumactl#test_control_gc_stats_(?:tcp|unix)" \
  "|TestRackUp::RackUp#test_(?:bin|rackup1)" \
  "|TestThreadPool#test_thread_name_linux)\\z/"
builtin = %w[bigdecimal json]

# A first lock straight from rubygems.org, for the versions; then the index
# from those versions, and the lock again against the index alone.
def bundle_lock(dir, source, env_dir)
  env = {
    "BUNDLE_GEMFILE" => File.join(dir, "Gemfile"),
    "BUNDLE_USER_HOME" => File.join(env_dir, "user"),
    "BUNDLE_APP_CONFIG" => File.join(env_dir, ".bundle")
  }
  system(env, "bundle", "lock", "--add-platform", "x86_64-linux", "aarch64-linux", "arm64-darwin",
    "x86_64-darwin", exception: true, out: File::NULL)
end

# The app's Gemfile names Puma by its gemspec, which loads `lib/puma/const`:
# a checkout's root for Bundler, upstream's `lib/` (puma-spinel's) beside it.
def checkout(here, dir)
  FileUtils.cp(File.join(here, "app/puma.gemspec"), dir)
  FileUtils.cp_r(File.join(here, "../../../../../gems/puma-spinel/lib"), dir)
end

locked = {}
Dir.mktmpdir do |dir|
  %w[app sinatra].each do |app|
    work = File.join(dir, app)
    FileUtils.mkdir_p(work)
    FileUtils.cp(File.join(here, app, "Gemfile"), work)
    checkout(here, work) if app == "app"
    start = app == "sinatra" ? File.join(here, "../sinatra-lock/Gemfile.lock") : nil
    FileUtils.cp(start, File.join(work, "Gemfile.lock")) if start
    bundle_lock(work, "https://rubygems.org", dir)
    locked.merge!(locked_specs(File.read(File.join(work, "Gemfile.lock"))))
  end
end

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
  next if substituted.include?(name) || builtin.include?(name)

  gem = "#{name}-#{version}.gem"
  next if File.exist?(File.join(gems_dir, gem))
  bytes = Net::HTTP.get(URI("https://rubygems.org/gems/#{gem}"))
  checksum = lines.first[/checksum:(\h+)/, 1]
  abort "build.rb: #{gem} is not the gem its checksum names" unless Digest::SHA256.hexdigest(bytes) == checksum
  File.binwrite(File.join(gems_dir, gem), bytes)
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

%w[app sinatra].each do |app|
  Dir.mktmpdir do |dir|
    File.write(File.join(dir, "Gemfile"), File.read(File.join(here, app, "Gemfile")).sub("https://rubygems.org", source))
    checkout(here, dir) if app == "app"
    if app == "sinatra"
      start = File.read(File.join(here, "../sinatra-lock/Gemfile.lock")).sub("https://rubygems.org/", "#{source}/")
      File.write(File.join(dir, "Gemfile.lock"), start)
    end
    bundle_lock(dir, source, dir)
    lock = File.read(File.join(dir, "Gemfile.lock")).sub("#{source}/", "https://rubygems.org/")
    File.write(File.join(here, app, "Gemfile.lock"), lock)
  end
end

# The locked gems from rubygems.org into `dir`, Puma's C extension
# compiled: the environment that runs them, and Puma's directory.
def install(dir)
  env = {
    "BUNDLE_GEMFILE" => File.join(dir, "Gemfile"),
    "BUNDLE_PATH" => File.join(dir, "bundle"),
    "BUNDLE_USER_HOME" => File.join(dir, "user"),
    "BUNDLE_APP_CONFIG" => File.join(dir, ".bundle")
  }
  system(env, "bundle", "install", "--quiet", exception: true)
  puma = IO.popen(env, ["bundle", "exec", "ruby", "-e", "print Gem.loaded_specs['puma'].full_gem_path"], &:read)
  [env, puma]
end

Dir.mktmpdir do |dir|
  FileUtils.cp(File.join(here, "sinatra/Gemfile"), dir)
  FileUtils.cp(File.join(here, "sinatra/Gemfile.lock"), dir)
  env, puma = install(dir)
  output = IO.popen(env, ["bundle", "exec", "ruby", File.join(here, "surface.rb")], err: [:child, :out],
    chdir: dir, &:read)
  File.write(File.join(here, "surface.txt"), output)

  port = TCPServer.open("127.0.0.1", 0) { |s| s.addr[1] }
  pid = Process.spawn(env.merge("PORT" => port.to_s), "bundle", "exec", "ruby", File.join(here, "sinatra/app.rb"),
    chdir: dir, out: File::NULL, err: File::NULL)
  begin
    response = nil
    100.times do
      response = TCPSocket.open("127.0.0.1", port) do |s|
        s.write("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        s.read
      end
      break
    rescue Errno::ECONNREFUSED
      sleep 0.1
    end
    File.binwrite(File.join(here, "sinatra.txt"), response)
  ensure
    Process.kill(:TERM, pid)
    Process.wait(pid)
  end

  # Puma's suite in a checkout as its `test/runner` expects one: `test/`,
  # `puma.gemspec`, and the installed gem's `lib/`, its extension compiled
  # into it, and `bin/` at the root.
  if suite
    project = File.join(dir, "project")
    FileUtils.mkdir_p(project)
    FileUtils.cp_r(File.join(here, "app/test"), project)
    FileUtils.cp(File.join(here, "app/puma.gemspec"), project)
    FileUtils.cp(File.join(here, "app/Gemfile"), project)
    FileUtils.cp(File.join(here, "app/Gemfile.lock"), project)
    FileUtils.cp_r(File.join(puma, "lib"), project)
    FileUtils.cp_r(File.join(puma, "bin"), project)
    run, = install(project)
    # The KeyUpdate client, as `tests/puma.rs` builds it.
    target = File.join(dir, "target")
    system("cargo", "build", "--quiet", "--package", "puma-spinel", "--example", "key_update_peer",
      "--manifest-path", File.join(here, "../../../../../Cargo.toml"), "--target-dir", target, exception: true)
    # `CI` turns on minitest-retry, which Puma's own CI runs its suite with
    # for the tests that race the server; the GitHub step summary it would
    # also write is off without `GITHUB_ACTIONS`.
    # `MT_CPU=1` runs the classes that ask to run their tests in parallel one
    # test at a time, as on a single CPU: their tests time servers against
    # fixed sleeps, and Spinel's threads of one Ractor take turns.
    results = File.join(dir, "results.txt")
    run = run.merge("CI" => "1", "GITHUB_ACTIONS" => nil, "GITHUB_STEP_SUMMARY" => nil, "MT_CPU" => "1",
      "PUMA_SUITE_RESULTS" => results,
      "PUMA_KEY_UPDATE_PEER" => File.join(target, "debug/examples/key_update_peer"))
    command = ["unshare", "-rn", "sh", "-c", 'ip link set lo up && exec "$@"', "sh",
      "ruby", "-rbundler/setup", "-r#{File.join(here, "results.rb")}",
      "-r#{File.join(here, "key_update_peer.rb")}", "-Ilib", "test/runner", "-v",
      "--exclude=#{EXCLUDED}"]
    out = IO.popen(run, command, chdir: project, err: File::NULL, &:read)
    File.write(ENV["PUMA_SUITE_LOG"], out) if ENV["PUMA_SUITE_LOG"]
    File.write(File.join(here, "suite.txt"), File.readlines(results).sort.join)
  end
end
