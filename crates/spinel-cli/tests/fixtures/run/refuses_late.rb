# A method whose body this build cannot compile yet: the file still runs, and
# only calling the method stops it. A complex literal waits on `Complex` (#227).
def later = 3i

puts "before"
later
puts "after"
