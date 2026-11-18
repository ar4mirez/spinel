# The built-in openssl (#106), phase 1, against ruby 4.0.9: every line this
# prints is what CRuby prints (openssl.out). Known-answer vectors are checked
# here too, against the values their standards publish, so a line reading
# MISMATCH is wrong on any Ruby. Nothing printed depends on the OpenSSL
# release: error messages are checked by their prefix, which is ruby/openssl's,
# not by the reason OpenSSL appends.
require "openssl"

def hex(s) = s.unpack1("H*")
def bytes(*list) = list.pack("C*")

def check(label, actual, expected)
  puts "#{actual == expected ? "ok      " : "MISMATCH"} #{label} #{actual}"
end

def try
  r = yield
  "=> #{r.inspect}"
rescue Exception => e
  "!! #{e.class}: #{e.message}"
end

# An OpenSSL error, by what ruby/openssl says, not what OpenSSL appends.
def failure(prefix)
  yield
  "no error"
rescue OpenSSL::OpenSSLError => e
  "!! #{e.class}: starts #{prefix.inspect}: #{e.message.start_with?(prefix)}, " \
    "errors: #{e.errors.class}, detailed: #{e.detailed_message.include?("OpenSSL error queue reported")}"
end

puts "== the gem"
puts OpenSSL::VERSION
puts OpenSSL::OPENSSL_VERSION.start_with?("OpenSSL 3.")
puts OpenSSL::OPENSSL_LIBRARY_VERSION.start_with?("OpenSSL 3.")
puts OpenSSL::OPENSSL_VERSION_NUMBER >= 0x3000_0000
puts OpenSSL::OPENSSL_FIPS
p OpenSSL::Digest.ancestors.take(3)
p OpenSSL::Digest::DigestError.ancestors.take(3)
p OpenSSL::HMACError.ancestors.take(3)
p OpenSSL::Random::RandomError.ancestors.take(3)
p OpenSSL::Digest.digests.include?("sha256"), OpenSSL::Digest.digests.include?("md5")

puts "== SHA-1, SHA-2 (FIPS 180 examples)"
abc = "abc"
two_block = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
million = "a" * 1_000_000
{
  ["SHA1", ""] => "da39a3ee5e6b4b0d3255bfef95601890afd80709",
  ["SHA1", abc] => "a9993e364706816aba3e25717850c26c9cd0d89d",
  ["SHA1", two_block] => "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
  ["SHA1", million] => "34aa973cd4c4daa4f61eeb2bdbad27316534016f",
  ["SHA256", ""] => "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  ["SHA256", abc] => "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  ["SHA256", two_block] => "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
  ["SHA256", million] => "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
  ["SHA224", abc] => "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7",
  ["SHA384", abc] => "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed" \
                     "8086072ba1e7cc2358baeca134c825a7",
  ["SHA512", abc] => "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a" \
                     "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
}.each do |(name, input), expected|
  check("#{name}(#{input.bytesize} bytes)", OpenSSL::Digest.hexdigest(name, input), expected)
end

puts "== MD5 (RFC 1321, A.5)"
{
  "" => "d41d8cd98f00b204e9800998ecf8427e",
  "a" => "0cc175b9c0f1b6a831c399e269772661",
  "abc" => "900150983cd24fb0d6963f7d28e17f72",
  "message digest" => "f96b697d7cb7938d525a2f31aaf161d0",
  "abcdefghijklmnopqrstuvwxyz" => "c3fcd3d76192e4007dfb496cca67e13b",
  "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789" =>
    "d174ab98d277d9f5a5611c2c9f419d9f",
  "1234567890" * 8 => "57edf4a22be3c955ac49da2e2107b67a",
}.each do |input, expected|
  check("MD5(#{input.inspect[0, 20]})", OpenSSL::Digest::MD5.hexdigest(input), expected)
end

puts "== HMAC-SHA256 (RFC 4231, 4.2 to 4.8)"
long_key = bytes(*[0xaa] * 131)
[
  [bytes(*[0x0b] * 20), "Hi There",
   "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"],
  ["Jefe", "what do ya want for nothing?",
   "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"],
  [bytes(*[0xaa] * 20), bytes(*[0xdd] * 50),
   "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"],
  [bytes(*(1..25).to_a), bytes(*[0xcd] * 50),
   "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"],
  # Test Case 5 publishes the MAC truncated to 128 bits.
  [bytes(*[0x0c] * 20), "Test With Truncation", "a3b6167473100ee06e0c796c2955552b"],
  [long_key, "Test Using Larger Than Block-Size Key - Hash Key First",
   "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"],
  [long_key, "This is a test using a larger than block-size key and a larger " \
             "than block-size data. The key needs to be hashed before being " \
             "used by the HMAC algorithm.",
   "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"],
].each_with_index do |(key, data, expected), index|
  mac = OpenSSL::HMAC.hexdigest("SHA256", key, data)
  check("test case #{index + 1}", mac[0, expected.length], expected)
end

puts "== HMAC-SHA1 and HMAC-MD5 (RFC 2202)"
check("HMAC-SHA1 test case 2", OpenSSL::HMAC.hexdigest("SHA1", "Jefe", "what do ya want for nothing?"),
      "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79")
check("HMAC-MD5 test case 2", OpenSSL::HMAC.hexdigest("MD5", "Jefe", "what do ya want for nothing?"),
      "750c783e6ab0b503eaa86e310a5db738")
check("HMAC-SHA1 test case 1", OpenSSL::HMAC.hexdigest(OpenSSL::Digest.new("SHA1"),
                                                      bytes(*[0x0b] * 20), "Hi There"),
      "b617318655057264e28bc0b6fb378c8ef146be00")

puts "== Digest objects"
d = OpenSSL::Digest.new("sha256")
p d.name, d.digest_length, d.block_length
p d
d << "a"
d.update("b")
copy = d.dup
d << "c"
p d.hexdigest, copy.hexdigest
p d.hexdigest.encoding, d.digest.encoding, d.digest.bytesize
p d.to_s == d.hexdigest, d == d.hexdigest, d == d.digest, d == OpenSSL::Digest.new("sha256", "abc")
p d.base64digest
p d.digest!.bytesize, d.hexdigest
d.update("abc")
p d.reset.equal?(d), d.hexdigest
p OpenSSL::Digest.new(OpenSSL::Digest::SHA1.new("ignored")).hexdigest
p OpenSSL::Digest::SHA512.new.name, OpenSSL::Digest::SHA384.new.digest_length
p OpenSSL::Digest::SHA1.digest("abc").bytesize, OpenSSL::Digest::SHA1.base64digest("abc")
p OpenSSL::Digest.digest("SHA256", "abc") == OpenSSL::Digest::SHA256.digest("abc")
p OpenSSL::Digest.base64digest("MD5", "abc")
p OpenSSL::Digest("SHA256"), OpenSSL::Digest::SHA256.new.class.superclass
p OpenSSL::Digest.new("RIPEMD160").name, OpenSSL::Digest.new("sha512-256").name
p OpenSSL::Digest.new("2.16.840.1.101.3.4.2.1").name
long = OpenSSL::Digest.new("sha1")
1000.times { |i| long << i.to_s }
p long.hexdigest
str = Object.new
def str.to_str = "abc"
p OpenSSL::Digest::SHA256.new.update(str).hexdigest == OpenSSL::Digest::SHA256.hexdigest("abc")
puts try { OpenSSL::Digest.new(:SHA1) }
puts try { OpenSSL::Digest.new(OpenSSL::Digest::SHA1) }
puts try { OpenSSL::Digest.new("sha1\0x") }
puts try { OpenSSL::Digest.new }
puts try { OpenSSL::Digest::SHA256.new.update(1) }
puts try { OpenSSL::Digest.allocate.update("x") }
puts try { OpenSSL::Digest.allocate.dup }
puts try { Marshal.dump(OpenSSL::Digest::SHA1.new) }
puts failure("unsupported digest algorithm: wd40") { OpenSSL::Digest.new("wd40") }

puts "== HMAC objects"
h = OpenSSL::HMAC.new("key", "SHA256")
h << "The quick brown fox "
copy = h.dup
h.update("jumps over the lazy dog")
p h.hexdigest, h.hexdigest, copy.hexdigest
p h.to_s == h.hexdigest, h.inspect == h.hexdigest, h.hexdigest.encoding, h.digest.encoding
p h.base64digest
p h == OpenSSL::HMAC.new("key", "SHA256").update("The quick brown fox jumps over the lazy dog"), h == copy, h == h.hexdigest
p h.reset.equal?(h), h.hexdigest
p OpenSSL::HMAC.digest("SHA256", "key", "x").bytesize, OpenSSL::HMAC.base64digest("SHA1", "key", "x")
p OpenSSL::HMAC.hexdigest("SHA256", "k\0e\xffy".b, "da\0ta")
p OpenSSL::HMAC.hexdigest("SHA256", "", "")
puts try { OpenSSL::HMAC.new(:key, "SHA1") }
puts try { OpenSSL::HMAC.new("key") }
puts try { OpenSSL::HMAC.new("key", "SHA1").update(nil) }
puts try { Marshal.dump(OpenSSL::HMAC.new("key", "SHA1")) }
puts failure("unsupported digest algorithm: wd40") { OpenSSL::HMAC.new("key", "wd40") }

puts "== Random"
[0, 1, 16, 64, 1000].each do |n|
  r = OpenSSL::Random.random_bytes(n)
  p [n, r.class, r.bytesize, r.encoding]
end
p OpenSSL::Random.pseudo_bytes(32).bytesize
draws = Array.new(64) { OpenSSL::Random.random_bytes(16) }
p draws.uniq.size
p OpenSSL::Random.random_bytes(2.9).bytesize
p OpenSSL::Random.status?
p OpenSSL::Random.seed("more entropy")
p OpenSSL::Random.random_add("more entropy", 1.5)
puts try { OpenSSL::Random.random_bytes(-1) }
puts try { OpenSSL::Random.random_bytes("1") }
puts try { OpenSSL::Random.random_bytes(nil) }
puts try { OpenSSL::Random.random_bytes(2**31) }
puts try { OpenSSL::Random.random_add("x", "1") }
p OpenSSL::Random.singleton_methods.sort

puts "== secure_compare"
p OpenSSL.fixed_length_secure_compare("abc", "abc"), OpenSSL.fixed_length_secure_compare("abc", "abd")
p OpenSSL.secure_compare("abc", "abc"), OpenSSL.secure_compare("abc", "abcd")
puts try { OpenSSL.fixed_length_secure_compare("abc", "abcd") }
puts try { OpenSSL.fixed_length_secure_compare("abc", :abc) }

# Defined on CRuby, whose C half has them all; undefined on Spinel until
# their phase lands, rather than standing in for them. Either way the line
# reads true.
puts "== what is not here yet is not here"
cruby = RUBY_ENGINE == "ruby"
%i[Cipher PKey X509 SSL BN KDF Buffering PKCS5].each do |name|
  p [name, OpenSSL.const_defined?(name, false) == cruby]
end
