# The client `TestPumaServerSSLKeyUpdate` floods Puma's server with TLS 1.3
# KeyUpdates from, built by cargo in place of the C one the test compiles
# with `RbConfig::CONFIG["CC"]`: Spinel builds no C and names no `CC`
# (#867). `gems/puma-spinel/ext/puma_spinel/examples/key_update_peer.rs` makes
# the C's OpenSSL calls in the C's order, so the test runs as it is and
# checks what it checks, on Spinel (`tests/puma.rs`) and on the CRuby run that
# recorded `suite.txt` (`build.rb --suite`) alike.
#
# Loaded with `-r` before Puma's `test/runner`, beside `results.rb`. Puma's
# test defines its `peer`, which compiles the C, when its file loads; this
# answers the client `PUMA_KEY_UPDATE_PEER` names in its place once every
# test file has loaded, as minitest starts its plugins, and with no
# `LD_LIBRARY_PATH` (the test's `peer_env`, for a C client linked against
# the OpenSSL CRuby was configured with): the client is linked against the
# system's, as puma-spinel is.
require "minitest"

module Minitest
  def self.plugin_puma_key_update_peer_init(_options)
    peer = ENV["PUMA_KEY_UPDATE_PEER"]
    return unless peer && defined?(::TestPumaServerSSLKeyUpdate)

    ::TestPumaServerSSLKeyUpdate.singleton_class.prepend(Module.new do
      define_method(:peer) { peer }
      define_method(:peer_env) { {} }
    end)
  end
end

Minitest.extensions << "puma_key_update_peer" unless Minitest.extensions.include?("puma_key_update_peer")
