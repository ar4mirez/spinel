# The app `crates/spinel-cli/tests/bundler_setup.rs` boots once `spinel
# install` has installed this directory's Gemfile.lock, under `spinel run`
# and, opt-in, under CRuby's own `bundle exec`: it boots as apps boot,
# through `bundler/setup`, serves two requests through a session middleware,
# and says which gems the bundle put on the load path and where Bundler read
# them from.
require "bundler/setup"
require "rack"
require "rack/mock_request"
require "rack/session"
require "rack/session/version"

app = Rack::Session::Pool.new(lambda do |env|
  session = env["rack.session"]
  session[:visits] = session.fetch(:visits, 0) + 1
  [200, {"content-type" => "text/plain"}, ["visit #{session[:visits]}"]]
end)
first = Rack::MockRequest.new(app).get("/")
cookie = first.headers["set-cookie"][/\Arack\.session=[^;]+/]
puts first.body
puts Rack::MockRequest.new(app).get("/", "HTTP_COOKIE" => cookie).body
puts "rack #{Rack.release}, rack-session #{Rack::Session::VERSION}"

bundle = Bundler.bundle_path.to_s
puts $LOAD_PATH.select { |path| path.start_with?(bundle) }.map { |path| path[%r{/gems/([^/]+)/}, 1] }.sort.join(" ")
puts Gem.loaded_specs.keys.sort.join(" ")
puts Bundler.bundle_path.relative_path_from(Pathname(__dir__))
