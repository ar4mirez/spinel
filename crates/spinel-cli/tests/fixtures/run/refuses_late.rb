# A method whose body this build cannot compile yet: the file still runs, and
# only calling the method stops it. A regexp standing alone as a condition
# matches against `$_`, which is not compiled.
def later = (1 if /a/)

puts "before"
later
puts "after"
