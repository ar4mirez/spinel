# The built-in openssl (#106), phase 1: a megabyte hashed whole and in 4 KiB
# updates, the per-call paths a program signs and checks through (a short
# String's SHA-256 and HMAC-SHA256, binary and hex), and random bytes.
#
#   target/release/spinel run bench/openssl.rb
#   ruby --yjit bench/openssl.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "openssl"

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(2)} ms"
end

megabyte = OpenSSL::Random.random_bytes(1 << 20)
pieces = (0...megabyte.bytesize).step(4096).map { |at| megabyte.byteslice(at, 4096) }
message = "GET /resource?user=42&nonce=0123456789abcdef"

time("SHA256 of 1 MB          ") { OpenSSL::Digest.digest("SHA256", megabyte) }
time("SHA256 of 1 MB, 4 KiB <<") do
  digest = OpenSSL::Digest.new("SHA256")
  pieces.each { |piece| digest << piece }
  digest.digest
end
time("HMAC-SHA256 of 1 MB     ") { OpenSSL::HMAC.digest("SHA256", "key", megabyte) }
time("10k SHA256.digest       ") { 10_000.times { OpenSSL::Digest::SHA256.digest(message) } }
time("10k HMAC.digest         ") { 10_000.times { OpenSSL::HMAC.digest("SHA256", "key", message) } }
time("10k HMAC.hexdigest      ") { 10_000.times { OpenSSL::HMAC.hexdigest("SHA256", "key", message) } }
time("10k random_bytes(16)    ") { 10_000.times { OpenSSL::Random.random_bytes(16) } }
