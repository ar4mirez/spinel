# A buffer that is not a String, nor converts to one, is a TypeError.
r, w = IO.pipe
w << "ab"
%i[read readpartial sysread read_nonblock].each do |m|
  r.public_send(m, 1, Object.new)
rescue => e
  p [m, e.class, e.message]
end
begin
  r.read(nil, 5)
rescue => e
  p [:read_all, e.class, e.message]
end
