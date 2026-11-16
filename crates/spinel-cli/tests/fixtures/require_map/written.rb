# A program that writes a file into a directory the require map lists, after
# `require` has looked there and not found it, and then requires it: every
# answer must be the one a run without the map gives.
WHICH = []
alpha = File.join(__dir__, "alpha/lib")
written = File.join(alpha, "written.rb")

$LOAD_PATH.unshift(alpha)
begin
  require "written"
rescue LoadError => e
  puts e.message
end
File.write(written, "WHICH << \"alpha written\"\n")
begin
  p require("written")
ensure
  File.delete(written)
end
puts WHICH.join(", ")
