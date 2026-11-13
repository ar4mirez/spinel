# The program the bytecode cache test runs twice (#40). Everything it prints
# comes from a required file, so a run that read the cache and one that
# parsed must print the same.
require_relative "lib/greeting"
require "./lib/failing"
require_relative "lib/conditional"

puts Greeting.hello("cache")
puts Greeting.where.inspect
begin
  Failing.call
rescue => e
  puts e.message
  puts e.backtrace.first(2)
end
puts Conditional.value
