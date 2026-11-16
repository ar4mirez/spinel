# The Sinatra app `crates/spinel-cli/tests/install.rs` serves once
# `spinel install` has installed this directory's Gemfile.lock: on WEBrick,
# on 127.0.0.1 and the port in `PORT`.
require "sinatra/base"

class App < Sinatra::Base
  set :server, "webrick"
  set :bind, "127.0.0.1"
  set :port, Integer(ENV.fetch("PORT"))
  set :logging, false
  set :quiet, true

  get "/" do
    "Hello from Sinatra #{Sinatra::VERSION}"
  end
end

App.run!
