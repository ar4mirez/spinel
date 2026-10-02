# `require_relative` loads a file once; `require` finds one on `$LOAD_PATH`.
p require_relative("lib/greeting")
p require_relative("lib/greeting")
puts Greeting.hello("spinel")
$LOAD_PATH.unshift(File.join(__dir__, "lib"))
p require("greeting")
p $LOADED_FEATURES.count { |f| f.end_with?("greeting.rb") }
begin
  require "no_such_library"
rescue LoadError => e
  puts e.message
end
p load(File.join(__dir__, "lib", "counter.rb"))
p load(File.join(__dir__, "lib", "counter.rb"))
p $loads
