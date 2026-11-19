# `openssl` phase 2 (#107): keys, certificates, stores and TLS over
# 127.0.0.1, every key and certificate made here, nothing read from disk but
# what this script writes. What it prints is what ruby 4.0.9 prints
# (`tls.out`); nothing printed depends on the keys made, the time, or the
# port the server got.

require "openssl"
require "socket"
require "net/https"
require "tmpdir"

def show(label)
  value = yield
  puts "#{label}: #{value.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.gsub(/:\d{4,5}\b/, ":PORT")}"
end

# --- keys ---------------------------------------------------------------

ca_key = OpenSSL::PKey::RSA.new(2048)
leaf_key = OpenSSL::PKey::RSA.new(2048)
show("key class") { [ca_key.class, ca_key.oid, ca_key.private?, ca_key.public?] }
show("public key") { pub = ca_key.public_key; [pub.class, pub.private?, pub.public?, pub.n == ca_key.n] }
show("pem") { [ca_key.to_pem.lines.first, ca_key.public_key.to_pem.lines.first, ca_key.private_to_pem.lines.first] }
show("read back") { [OpenSSL::PKey.read(ca_key.to_pem).to_der == ca_key.to_der, OpenSSL::PKey::RSA.new(ca_key.to_der).class] }
show("n bits") { [ca_key.n.num_bits, ca_key.e.to_i] }
show("params") { ca_key.params.keys }
sig = ca_key.sign("SHA256", "data")
show("sign/verify") { [sig.bytesize, ca_key.verify("SHA256", sig, "data"), ca_key.public_key.verify("SHA256", sig, "other")] }
show("compare") { [ca_key.compare?(ca_key.public_key), ca_key.compare?(leaf_key)] }
show("sign with public") { ca_key.public_key.sign("SHA256", "x") }
show("garbage key") { OpenSSL::PKey.read("garbage") }
show("garbage rsa") { OpenSSL::PKey::RSA.new("garbage") }
show("bad option") { OpenSSL::PKey.generate_key("RSA", "rsa_keygen_bits" => "x") }

# --- names --------------------------------------------------------------

name = OpenSSL::X509::Name.parse("/DC=org/DC=example/CN=Spinel Test CA")
show("name") { [name.to_s, name.to_a, name.inspect] }
show("name rfc2253") { [name.to_s(OpenSSL::X509::Name::RFC2253), name.to_utf8] }
show("name cmp") { [name == OpenSSL::X509::Name.parse(name.to_s), name <=> OpenSSL::X509::Name.parse("/CN=a"), name.hash == OpenSSL::X509::Name.new(name.to_der).hash] }
show("name comma") { OpenSSL::X509::Name.parse("CN=a, O=b").to_s }
show("name bad key") { OpenSSL::X509::Name.parse("hello=goodbye") }
show("name no pairs") { OpenSSL::X509::Name.parse("hello") }

# --- certificates ---------------------------------------------------------

now = Time.at(Time.now.to_i)
ca = OpenSSL::X509::Certificate.new
ca.version = 2
ca.serial = 1
ca.subject = name
ca.issuer = name
ca.public_key = ca_key.public_key
ca.not_before = now - 60
ca.not_after = now + 3600
ef = OpenSSL::X509::ExtensionFactory.new
ef.subject_certificate = ca
ef.issuer_certificate = ca
ca.add_extension(ef.create_extension("basicConstraints", "CA:TRUE", true))
ca.add_extension(ef.create_extension("keyUsage", "keyCertSign, cRLSign", true))
ca.add_extension(ef.create_extension("subjectKeyIdentifier", "hash", false))
ca.add_extension(ef.create_extension("authorityKeyIdentifier", "keyid:always", false))
ca.sign(ca_key, OpenSSL::Digest.new("SHA256"))

leaf = OpenSSL::X509::Certificate.new
leaf.version = 2
leaf.serial = 0x1234_5678_9abc_def0_1234
leaf.subject = OpenSSL::X509::Name.new([["CN", "localhost"]])
leaf.issuer = ca.subject
leaf.public_key = leaf_key.public_key
leaf.not_before = now - 60
leaf.not_after = now + 3600
ef = OpenSSL::X509::ExtensionFactory.new(ca, leaf)
leaf.add_extension(ef.create_extension("basicConstraints", "CA:FALSE"))
leaf.add_extension(ef.create_extension("keyUsage", "digitalSignature, keyEncipherment", true))
leaf.add_extension(ef.create_extension("extendedKeyUsage", "serverAuth"))
leaf.add_extension(ef.create_extension("subjectAltName", "DNS:localhost, IP:127.0.0.1"))
leaf.sign(ca_key, "SHA256")

show("cert fields") { [leaf.version, leaf.serial.to_i, leaf.serial.class, leaf.subject.to_s, leaf.issuer.to_s, leaf.signature_algorithm] }
show("cert times") { [leaf.not_before == now - 60, leaf.not_after.utc?, leaf.not_after - leaf.not_before] }
show("cert extensions") { leaf.extensions.map { |e| [e.oid, e.critical?] } }
show("san") { leaf.extensions.find { |e| e.oid == "subjectAltName" }.value }
show("ca extension") { ca.extensions.first.to_s }
show("signed by") { [leaf.verify(ca_key.public_key), leaf.verify(leaf_key.public_key), leaf.check_private_key(leaf_key), leaf.check_private_key(ca_key)] }
show("public key back") { leaf.public_key.to_der == leaf_key.public_key.to_der }
show("pem round trip") { c = OpenSSL::X509::Certificate.new(leaf.to_pem); [c == leaf, c.to_der == leaf.to_der, c.subject.to_s] }
show("der round trip") { OpenSSL::X509::Certificate.new(leaf.to_der) == leaf }
show("load") { OpenSSL::X509::Certificate.load(ca.to_pem + leaf.to_pem).map { |c| c.subject.to_s } }
show("garbage cert") { OpenSSL::X509::Certificate.new("garbage") }
show("empty cert") { c = OpenSSL::X509::Certificate.new; [c.version, c.serial.to_i, c.subject.to_s] }
show("empty not_before") { OpenSSL::X509::Certificate.new.not_before }
show("identity") { [OpenSSL::SSL.verify_certificate_identity(leaf, "localhost"), OpenSSL::SSL.verify_certificate_identity(leaf, "127.0.0.1"), OpenSSL::SSL.verify_certificate_identity(leaf, "example.com")] }
show("extension new") { e = OpenSSL::X509::Extension.new("basicConstraints", OpenSSL::ASN1::Sequence([OpenSSL::ASN1::Boolean(true)])); [e.oid, e.value, e.critical?, e.to_der] }
show("extension bad oid") { OpenSSL::X509::Extension.new("nope", "x") }
# The message ends with OpenSSL's last queued reason, which OpenSSL 3.5
# and 3.6 queue differently here; the first queued is the same.
show("factory bad value") do
  OpenSSL::X509::ExtensionFactory.new.create_extension("basicConstraints", "garbage")
rescue OpenSSL::X509::ExtensionError => e
  [e.class, e.message.start_with?("basicConstraints = garbage: "), e.errors.first]
end

# --- stores ---------------------------------------------------------------

store = OpenSSL::X509::Store.new
store.add_cert(ca)
show("store verify") { [store.verify(leaf), store.error, store.error_string, store.chain.map { |c| c.subject.to_s }] }
show("empty store") { s = OpenSSL::X509::Store.new; [s.verify(leaf), s.error, s.error_string] }
seen = []
show("callback sees") do
  s = OpenSSL::X509::Store.new
  s.add_cert(ca)
  r = s.verify(leaf) { |ok, ctx| seen << [ok, ctx.error, ctx.error_depth, ctx.current_cert.subject.to_s]; ok }
  [r, seen]
end
show("callback overrides") do
  s = OpenSSL::X509::Store.new
  [s.verify(leaf) { |ok, ctx| true }, s.error]
end
show("callback rejects") do
  s = OpenSSL::X509::Store.new
  s.add_cert(ca)
  [s.verify(leaf) { |ok, ctx| false }, s.error, s.error_string]
end
show("callback sets error") do
  s = OpenSSL::X509::Store.new
  s.add_cert(ca)
  [s.verify(leaf) { |ok, ctx| ctx.error = OpenSSL::X509::V_ERR_CERT_REVOKED; false }, s.error]
end
show("store add_file missing") { OpenSSL::X509::Store.new.add_file("/nonexistent/spinel-ca.pem") }
show("store context") { c = OpenSSL::X509::StoreContext.new(store, leaf); [c.verify, c.error, c.chain.size, c.current_cert.subject.to_s] }

# --- TLS over 127.0.0.1 ----------------------------------------------------

def server_context(cert, key, version = nil)
  ctx = OpenSSL::SSL::SSLContext.new
  ctx.cert = cert
  ctx.key = key
  if version
    ctx.min_version = version
    ctx.max_version = version
  end
  ctx
end

# One connection: the server accepts, answers one line with "echo: " and
# the line, and closes. The block is the client, given the port.
def serve(ctx)
  server = TCPServer.new("127.0.0.1", 0)
  port = server.local_address.ip_port
  thread = Thread.new do
    sock = server.accept
    ssl = OpenSSL::SSL::SSLSocket.new(sock, ctx)
    ssl.sync_close = true
    begin
      ssl.accept
      line = ssl.gets
      ssl.write("echo: #{line}")
      ssl.close
      :served
    rescue OpenSSL::SSL::SSLError, SystemCallError, EOFError, IOError
      sock.close unless sock.closed?
      :refused
    end
  ensure
    server.close
  end
  yield port
ensure
  thread&.value
end

def client(port, ctx, hostname)
  sock = TCPSocket.new("127.0.0.1", port)
  ssl = OpenSSL::SSL::SSLSocket.new(sock, ctx)
  ssl.sync_close = true
  ssl.hostname = hostname if hostname
  begin
    ssl.connect
    yield ssl
  ensure
    ssl.close unless ssl.closed?
  end
end

server_ctx = server_context(leaf, leaf_key)

show("round trip") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    client(port, ctx, "localhost") do |ssl|
      ssl.post_connection_check("localhost")
      ssl.puts("hello over TLS")
      [ssl.ssl_version, ssl.verify_result, ssl.peer_cert.subject.to_s, ssl.peer_cert_chain.size,
       ssl.cipher.size, ssl.gets, ssl.hostname, ssl.session.class, ssl.state]
    end
  end
end

show("untrusted") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params
    client(port, ctx, "localhost") { |ssl| ssl.gets }
  end
end

show("hostname mismatch") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    client(port, ctx, "example.com") { |ssl| ssl.gets }
  end
end

show("post_connection_check") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store, verify_hostname: false)
    client(port, ctx, "localhost") do |ssl|
      ssl.puts("x")
      ssl.gets
      ssl.post_connection_check("example.com")
    end
  end
end

show("verify_callback") do
  calls = []
  result = serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    ctx.verify_callback = ->(ok, sctx) { calls << [ok, sctx.error, sctx.error_depth, sctx.current_cert.subject.to_s]; ok }
    client(port, ctx, "localhost") { |ssl| ssl.puts("cb"); ssl.gets }
  end
  [result, calls]
end

show("verify_callback accepts untrusted") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params
    ctx.verify_callback = ->(ok, sctx) { true }
    client(port, ctx, "localhost") { |ssl| ssl.puts("anyway"); [ssl.gets, ssl.verify_result] }
  end
end

show("verify_callback rejects") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    ctx.verify_callback = ->(ok, sctx) { false }
    client(port, ctx, "localhost") { |ssl| ssl.gets }
  end
end

show("verify none") do
  serve(server_ctx) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    client(port, ctx, nil) { |ssl| ssl.puts("plain"); [ssl.gets, ssl.verify_result] }
  end
end

show("connect_nonblock") do
  serve(server_ctx) do |port|
    sock = TCPSocket.new("127.0.0.1", port)
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    ssl = OpenSSL::SSL::SSLSocket.new(sock, ctx)
    ssl.hostname = "localhost"
    ssl.sync_close = true
    waits = []
    loop do
      case (r = ssl.connect_nonblock(exception: false))
      when :wait_readable then waits << r; sock.wait_readable
      when :wait_writable then waits << r; sock.wait_writable
      else break
      end
    end
    ssl.write("nonblock\n")
    out = [waits.include?(:wait_readable), ssl.gets]
    ssl.close
    out
  end
end

show("read before start") { r, w = IO.pipe; OpenSSL::SSL::SSLSocket.new(r).sysread(1) }
show("socket on a non-IO") { OpenSSL::SSL::SSLSocket.new(Object.new) }
show("context frozen") { ctx = OpenSSL::SSL::SSLContext.new; OpenSSL::SSL::SSLSocket.new(IO.pipe[0], ctx); [ctx.frozen?, ctx.setup] }
show("failed setup is tried again") do
  ctx = OpenSSL::SSL::SSLContext.new
  ctx.set_params(ca_file: File.join(Dir.tmpdir, "spinel-no-such-ca.pem"))
  tries = 2.times.map do
    OpenSSL::SSL::SSLSocket.new(IO.pipe[0], ctx)
    :set_up
  rescue OpenSSL::SSL::SSLError => e
    e.class
  end
  [tries, ctx.frozen?]
end

show("servername_cb context is set up") do
  sni_ctx = server_context(leaf, leaf_key)
  bare = OpenSSL::SSL::SSLContext.new
  bare.servername_cb = ->((_ssl, name)) { sni_ctx }
  serve(bare) do |port|
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.set_params(cert_store: store)
    client(port, ctx, "localhost") { |ssl| ssl.puts("sni"); [ssl.gets, ssl.peer_cert.subject.to_s, sni_ctx.frozen?] }
  end
end

show("bad version") { OpenSSL::SSL::SSLContext.new.min_version = :TLS9 }
show("bad ciphers") { OpenSSL::SSL::SSLContext.new.ciphers = "nope" }
show("default params") { c = OpenSSL::SSL::SSLContext.new; c.set_params; [c.verify_mode, c.verify_hostname, c.cert_store.equal?(OpenSSL::SSL::SSLContext::DEFAULT_CERT_STORE)] }

# --- what nio4r's SSLSocket spec does, on both TLS versions ----------------

[[:TLS1_2, "TLSv1.2"], [:TLS1_3, "TLSv1.3"]].each do |version, name|
  ctx = server_context(leaf, leaf_key, version)
  server = TCPServer.new("127.0.0.1", 0)
  client = TCPSocket.new("127.0.0.1", server.local_address.ip_port)
  peer = server.accept
  ssl_peer = OpenSSL::SSL::SSLSocket.new(peer, ctx)
  ssl_peer.sync_close = true
  ssl_client = OpenSSL::SSL::SSLSocket.new(client)
  ssl_client.sync_close = true
  thread = Thread.new { ssl_client.connect }
  ssl_peer.accept
  show("#{name} connect") { [thread.value.equal?(ssl_client), ssl_client.ssl_version, ssl_peer.ssl_version] }
  show("#{name} unreadable") { [ssl_client.read_nonblock(1, exception: false), IO.select([ssl_client], [], [], 0)] }
  ssl_peer << "data"
  ssl_peer.flush
  show("#{name} readable") { [IO.select([ssl_client], [], [], 10)&.first&.size, ssl_client.read_nonblock(4)] }
  show("#{name} writable") { IO.select([], [ssl_client], [], 1)[1].size }
  # Writes no one reads fill the socket, and then one would block.
  blocked =
    begin
      loop { ssl_client.write_nonblock("X" * 65536) }
    rescue OpenSSL::SSL::SSLError => e
      [e.class, e.is_a?(IO::WaitWritable), e.message]
    end
  show("#{name} fills") { blocked }
  ssl_peer.close
  show("#{name} peer closed") { IO.select([ssl_client], [], [], 1)&.first&.size }
  ssl_client.close
  server.close
end

# --- net/https ------------------------------------------------------------

def https_server(ctx)
  server = TCPServer.new("127.0.0.1", 0)
  port = server.local_address.ip_port
  thread = Thread.new do
    sock = server.accept
    ssl = OpenSSL::SSL::SSLSocket.new(sock, ctx)
    ssl.sync_close = true
    begin
      ssl.accept
      request = []
      while (line = ssl.gets) && line != "\r\n"
        request << line.chomp
      end
      body = "hello from #{request.first}"
      ssl.write("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
      ssl.close
    rescue OpenSSL::SSL::SSLError, SystemCallError, EOFError, IOError
      sock.close unless sock.closed?
    end
  ensure
    server.close
  end
  yield port
ensure
  thread&.join
end

Dir.mktmpdir do |dir|
  ca_file = File.join(dir, "ca.pem")
  File.write(ca_file, ca.to_pem)
  show("net/https get") do
    https_server(server_ctx) do |port|
      http = Net::HTTP.new("localhost", port)
      http.use_ssl = true
      http.ca_file = ca_file
      http.verify_mode = OpenSSL::SSL::VERIFY_PEER
      res = http.start { |h| h.get("/spinel") }
      [res.code, res["content-type"], res.body]
    end
  end
  show("net/https by address") do
    https_server(server_ctx) do |port|
      http = Net::HTTP.new("127.0.0.1", port)
      http.use_ssl = true
      http.ca_file = ca_file
      res = http.start { |h| h.get("/by-ip") }
      [res.code, res.body]
    end
  end
  show("net/https untrusted") do
    https_server(server_ctx) do |port|
      http = Net::HTTP.new("localhost", port)
      http.use_ssl = true
      http.start { |h| h.get("/") }
    end
  end
end
