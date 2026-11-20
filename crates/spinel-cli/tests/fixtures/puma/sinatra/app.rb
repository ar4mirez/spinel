# The Sinatra app `crates/spinel-cli/tests/puma.rs` serves once `spinel
# install` has installed this directory's Gemfile.lock: on Puma, on
# 127.0.0.1 and the port in `PORT`. `../sinatra-lock`'s app, on Puma in
# WEBrick's place.
require "sinatra/base"

class App < Sinatra::Base
  set :server, "puma"
  set :bind, "127.0.0.1"
  set :port, Integer(ENV.fetch("PORT"))
  set :logging, false
  set :quiet, true
  set :server_settings, {Silent: true}

  get "/" do
    "Hello from Sinatra #{Sinatra::VERSION} on Puma #{Puma::Const::PUMA_VERSION}"
  end
end

App.run!
