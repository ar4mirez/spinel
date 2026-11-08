# One WEBrick server on 127.0.0.1, on a port the system picks, answering
# requests from Net::HTTP over one kept-alive connection: a proc servlet, a
# form POST, a static file (GET and HEAD), a 404 and a raised status. Every
# line it prints is what ruby 4.0.9 prints for it. The server is shut down
# whatever happens; the test kills the process if even that hangs.
require "stringio"
require "webrick"
require "net/http"

log = StringIO.new
access = StringIO.new
server = WEBrick::HTTPServer.new(
  BindAddress: "127.0.0.1",
  Port: 0,
  Logger: WEBrick::Log.new(log, WEBrick::BasicLog::WARN),
  AccessLog: [[access, "%m %U %s %b"]],
  DocumentRoot: File.join(__dir__, "root"),
)
server.mount_proc("/hello") do |req, res|
  res["Content-Type"] = "text/plain"
  res.body = "hello #{req.query["name"]} via #{req.request_method}"
end
server.mount_proc("/echo") do |req, res|
  res.status = 201
  res["Content-Type"] = "application/json"
  res["X-Seen"] = req["X-Custom"]
  res.cookies << WEBrick::Cookie.new("k", "v")
  res.body = "#{req.content_type}|#{req.body}|#{req.query.sort.inspect}"
end
server.mount_proc("/boom") { |_req, _res| raise WEBrick::HTTPStatus::Forbidden, "no" }

port = server.config[:Port]
puts "listening: #{server.listeners.map { |s| s.local_address.ip_address }.inspect} #{port > 0}"
thread = Thread.new { server.start }
begin
  Net::HTTP.start("127.0.0.1", port, open_timeout: 10, read_timeout: 10) do |http|
    res = http.get("/hello?name=spinel")
    puts [res.code, res["content-type"], res["content-length"], res["connection"], res.body].inspect
    puts res["server"].split(" ").first
    form = { "Content-Type" => "application/x-www-form-urlencoded", "X-Custom" => "yes" }
    res = http.post("/echo", "a=1&b=two", form)
    puts [res.code, res["content-type"], res["x-seen"], res["set-cookie"], res.body].inspect
    res = http.get("/index.txt")
    puts [res.code, res["content-type"], res["content-length"], res.body].inspect
    res = http.head("/index.txt")
    puts [res.code, res["content-length"], res.body].inspect
    res = http.get("/missing")
    puts [res.code, res["content-type"], res.body[/<TITLE>(.*)<\/TITLE>/, 1]].inspect
    res = http.get("/boom")
    puts [res.code, res.body[/<TITLE>(.*)<\/TITLE>/, 1]].inspect
  end
ensure
  server.shutdown
  thread.join(10)
end
puts access.string
puts "status: #{server.status}"
puts "listeners: #{server.listeners.size}"
puts log.string.lines.map { |line| line.sub(/\A\[[^\]]*\] /, "") }
