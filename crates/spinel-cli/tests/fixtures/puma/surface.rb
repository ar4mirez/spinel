# What a program sees of the puma gem's C extension, `puma_http11`: each line
# one call's answer or the exception it raised, class and message.
# `surface.txt` is what CRuby 4.0 printed with upstream Puma 8.1.2, its C
# extension compiled (`build.rb` records it); `crates/spinel-cli/tests/puma.rs`
# runs this on Spinel with puma-spinel installed in puma's place and
# compares, line for line.
#
# What depends on the machine is printed as whether it is what it should be:
# the OpenSSL version names, and the text OpenSSL's error queue adds to a
# message, which is that OpenSSL's; object addresses are masked.
require "puma"
require "puma/minissl"
require "tmpdir"

# One line, an object's address masked.
def show(label)
  v = yield
  puts "#{label}: #{v.inspect}".gsub(/0x\h{8,}/, "0x")
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.inspect}".gsub(/0x\h{8,}/, "0x")
end

# A request parsed from the start: the Hash, what it answered, and the state.
def parse(data, from = 0, env = {})
  parser = Puma::HttpParser.new
  nread = parser.execute(env, data, from)
  [nread, parser.finished?, parser.error?, parser.nread, parser.body, env]
rescue Puma::HttpParserError => e
  [e.class, e.message, parser.finished?, parser.error?, parser.nread, env]
end

# Each value's encoding and whether it is frozen, after the value.
def described(env)
  env.map { |k, v| [k, k.encoding.to_s, k.frozen?, v, v.encoding.to_s, v.frozen?] }
end

show("classes") { [Puma::HttpParser.superclass, Puma::HttpParserError.superclass, Puma::HttpParser.instance_method(:execute).arity] }
show("methods") { Puma::HttpParser.public_instance_methods(false).sort }
show("ivars") { Puma::HttpParser.new.instance_variables }
show("new args") { Puma::HttpParser.new(1) }
show("allocate") { p = Puma::HttpParser.allocate; [p.nread, p.finished?, p.error?, p.body] }
show("fresh") { p = Puma::HttpParser.new; [p.nread, p.finished?, p.error?, p.body, p.finish, p.reset] }

show("get") { parse(+"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n") }
show("encodings") { described(parse(+"GET /a?b=c#d HTTP/1.1\r\nX-Odd_Name: v\r\nHost: h\r\n\r\n")[5]) }
show("full uri") { parse(+"GET http://h:80/p/q;r?s=t HTTP/1.0\r\n\r\n") }
show("asterisk") { parse(+"OPTIONS * HTTP/1.1\r\n\r\n") }
show("body") { parse(+"POST /x HTTP/1.1\r\nContent-Length: 4\r\nContent-Type: a/b\r\n\r\nbodyextra") }
show("duplicates") { parse(+"GET / HTTP/1.1\r\nA: 1\r\nB: 2\r\na: 3\r\nA:4\r\n\r\n") }
show("into existing") { parse(+"GET / HTTP/1.1\r\nCookie: b\r\n\r\n", 0, {"HTTP_COOKIE" => +"a"}) }
show("whitespace") { parse(+"GET / HTTP/1.1\r\nX-Pad: \t  v  v \t\r\nX-Empty:\r\n\r\n") }
show("tab value") { parse(+"GET / HTTP/1.1\r\nX: a\tb\r\n\r\n") }
show("snake") { data = +"GET / HTTP/1.1\r\nx-a_b-c: v\r\n\r\n"; [parse(data)[5], data] }
# The C also upcases a frozen String's bytes in place, which Ruby cannot
# (`puma_http11.rb`'s `ponytail:`): only what it parsed is compared.
show("frozen data") { parse("GET / HTTP/1.1\r\nx-y: v\r\n\r\n".freeze)[5] }
show("partial") { parse(+"GET / HTTP/1.1\r\nHost: h\r\n") }
show("partial line") { parse(+"GET /pa") }
show("lowercase method") { parse(+"get / HTTP/1.1\r\n\r\n") }
show("long method") { parse(+"#{"A" * 21} / HTTP/1.1\r\n\r\n") }
show("bad protocol") { parse(+"GET / HTTP/x\r\n\r\n") }
show("bare lf") { parse(+"GET / HTTP/1.1\nHost: h\n\n") }
show("space in name") { parse(+"GET / HTTP/1.1\r\nBad Name: v\r\n\r\n") }
show("ctl in value") { parse(+"GET / HTTP/1.1\r\nX: a\x01b\r\n\r\n") }
show("high bytes") { parse(+"GET /\xC3\xA9 HTTP/1.1\r\nX: \xFF\r\n\r\n".b) }
show("unwise path") { parse(+"GET /{a} HTTP/1.1\r\n\r\n") }
show("unwise query") { parse(+"GET /a?{b} HTTP/1.1\r\n\r\n") }
show("ssl to plain") { parse("\x16\x03\x01\x02\x00\x01".b) }
show("name 256") { parse(+"GET / HTTP/1.1\r\n#{"N" * 256}: v\r\n\r\n")[0] }
show("name 257") { parse(+"GET / HTTP/1.1\r\n#{"N" * 257}: v\r\n\r\n") }
show("value 80k") { parse(+"GET / HTTP/1.1\r\nX: #{"v" * (80 * 1024 + 1)}\r\n\r\n").first(5) }
show("uri") { parse(+"GET /#{"u" * (12 * 1024)} HTTP/1.1\r\n\r\n").first(5) }
show("path") { parse(+"GET /#{"p" * 8192} HTTP/1.1\r\n\r\n").first(5) }
show("query") { parse(+"GET /?#{"q" * (10 * 1024 + 1)} HTTP/1.1\r\n\r\n").first(5) }
show("fragment") { parse(+"GET /##{"f" * 1025} HTTP/1.1\r\n\r\n").first(5) }
show("header") { parse(+"GET / HTTP/1.1\r\n#{"X-Test: test\r\n" * 8000}\r\n").first(5) }
show("start at end") { Puma::HttpParser.new.execute({}, +"GET", 3) }
show("start past end") { Puma::HttpParser.new.execute({}, +"GET", 9) }
show("empty data") { Puma::HttpParser.new.execute({}, +"", 0) }

show("in pieces") do
  parser = Puma::HttpParser.new
  env = {}
  whole = +"GET /a?b HTTP/1.1\r\nHost: h\r\nX-Long-Name: value\r\n\r\nrest"
  data = +""
  answers = [5, 17, 23, 31, whole.bytesize].map do |upto|
    data << whole.byteslice(data.bytesize, upto - data.bytesize)
    [parser.execute(env, data, parser.nread), parser.finished?]
  end
  [answers, env, data, parser.body]
end
show("reused") do
  parser = Puma::HttpParser.new
  first = parser.execute({}, +"GET / HTTP/1.1\r\n\r\n", 0)
  again = parser.execute({}, +"GET / HTTP/1.1\r\n\r\n", 0)
  parser.reset
  [first, again, parser.nread, parser.finished?, parser.execute({}, +"GET /x HTTP/1.1\r\n\r\n", 0)]
end
show("after error") do
  parser = Puma::HttpParser.new
  begin
    parser.execute({}, +"BAD LINE\r\n\r\n", 0)
  rescue Puma::HttpParserError
  end
  [parser.error?, parser.finish, parser.nread, (parser.execute({}, +"GET / HTTP/1.1\r\n\r\n", 0) rescue $!.message)]
end
show("dup") do
  parser = Puma::HttpParser.new
  parser.execute({}, +"GET / HTTP/1.1\r\n\r\nb", 0)
  copy = parser.dup
  [copy.nread, copy.finished?, copy.body, parser.nread, parser.body]
end
show("execute types") { Puma::HttpParser.new.execute({}, +"GET / HTTP/1.1\r\n\r\n", 0.0) }

show("subreaper") { Puma.respond_to?(:enable_child_subreaper) }

show("ssl classes") { [Puma::MiniSSL::SSLError.superclass, Puma::MiniSSL::SSLContext.superclass, Puma::MiniSSL::Engine.superclass] }
show("ssl versions") do
  [Puma::MiniSSL::OPENSSL_VERSION.start_with?("OpenSSL "), Puma::MiniSSL::OPENSSL_LIBRARY_VERSION.start_with?("OpenSSL "),
    Puma::MiniSSL::OPENSSL_VERSION.encoding, Puma::MiniSSL::OPENSSL_NO_SSL3, Puma::MiniSSL::OPENSSL_NO_TLS1,
    Puma::MiniSSL::OPENSSL_NO_TLS1_1]
end
show("check") { Puma::MiniSSL.check }
show("engine new") { Puma::MiniSSL::Engine.new }
show("engine allocate") { Puma::MiniSSL::Engine.allocate }
show("sslcontext dup") { Puma::MiniSSL::SSLContext.instance_method(:dup).owner }

certs = File.expand_path("app/test/ssl", __dir__)
certs = File.expand_path("test/ssl", Dir.pwd) unless File.directory?(certs)
context = lambda do |**settings|
  ctx = Puma::MiniSSL::Context.new
  ctx.key = "#{certs}/puma_keypair.pem"
  ctx.cert = "#{certs}/cert_puma.pem"
  settings.each { |k, v| ctx.public_send(:"#{k}=", v) }
  ctx
end
# A message up to the OpenSSL error string it ends with, which is the
# library's own text.
masked = ->(e) { "#{e.class}: #{e.message.sub(/(: error|:)[^:]*:.*\z/m, '\1...')}" }
show("sslcontext") { c = Puma::MiniSSL::SSLContext.new(context.call); [c.frozen?, c.instance_variables] }
show("sslcontext verify") { Puma::MiniSSL::SSLContext.new(context.call(verify_mode: Puma::MiniSSL::VERIFY_PEER)).frozen? }
show("bad cert") do
  ctx = context.call
  ctx.instance_variable_set(:@cert, "#{certs}/missing.pem")
  Puma::MiniSSL::SSLContext.new(ctx)
rescue => e
  masked.(e)
end
show("bad key") do
  ctx = context.call
  ctx.instance_variable_set(:@key, "#{certs}/cert_puma.pem")
  Puma::MiniSSL::SSLContext.new(ctx)
rescue => e
  masked.(e)
end
show("bad ciphers") do
  Puma::MiniSSL::SSLContext.new(context.call(ssl_cipher_filter: "NOPE-NOT-A-CIPHER")).frozen?
rescue => e
  masked.(e)
end
show("pem") do
  ctx = Puma::MiniSSL::Context.new
  ctx.cert_pem = File.read("#{certs}/cert_puma.pem")
  ctx.key_pem = File.read("#{certs}/puma_keypair.pem")
  Puma::MiniSSL::SSLContext.new(ctx).frozen?
end
show("bad pem") do
  ctx = Puma::MiniSSL::Context.new
  ctx.cert_pem = "not a pem"
  ctx.key_pem = File.read("#{certs}/puma_keypair.pem")
  Puma::MiniSSL::SSLContext.new(ctx)
rescue => e
  masked.(e)
end

show("engine") do
  e = Puma::MiniSSL::Engine.server(Puma::MiniSSL::SSLContext.new(context.call))
  [e.init?, e.peercert, e.read, e.extract, e.ssl_vers_st, e.shutdown, e.instance_variables]
end
show("engine garbage") do
  e = Puma::MiniSSL::Engine.server(Puma::MiniSSL::SSLContext.new(context.call))
  [e.inject("not a client hello at all".b), (e.read rescue masked.($!))]
end
show("engine inject types") { Puma::MiniSSL::Engine.server(Puma::MiniSSL::SSLContext.new(context.call)).inject(1) }
show("engine client") { e = Puma::MiniSSL::Engine.client; [e.init?, e.extract.class, e.extract.nil?] }
