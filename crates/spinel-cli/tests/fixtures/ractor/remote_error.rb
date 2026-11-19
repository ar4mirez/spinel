r = Ractor.new { raise "bad input" }
begin
  r.value
rescue Ractor::RemoteError => e
  p e.class, e.message
  p e.cause.class, e.cause.message
  p e.ractor == r
end
begin
  r.join
rescue Ractor::RemoteError => e
  p e.cause.message
end
