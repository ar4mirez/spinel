# A method whose body this build cannot compile yet: the file still runs, and
# only calling the method stops it. A rational literal waits on #227.
def later = 3r

puts "before"
later
puts "after"
