# A stand-in for `openssl`, for crates/spinel-cli/tests/install.rs only: it
# is put on the load path with `-I` by the test that serves the Sinatra
# fixture, and nowhere else.
#
# Sinatra::Base names Rack::Session::Cookie in its class body, and
# rack-session's cookie.rb starts with `require 'openssl'`, which Spinel
# does not have yet. These are the constants rack-session reads while it
# loads; nothing here encrypts or signs, and the app the test serves keeps
# sessions off, so nothing calls them.
#
# ponytail: a stand-in so the installed Sinatra can serve before openssl
# exists. Upgrade: delete this file, and the test's `-I`, when openssl
# lands (#106).
module OpenSSL
  class OpenSSLError < StandardError; end

  class Cipher
    class CipherError < OpenSSLError; end
  end

  class HMAC; end

  module Digest
    class SHA256; end
  end
end
