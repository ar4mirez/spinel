# `String#upcase` needs case mapping, which is the Encoding slice's. Until then
# this file is how the message a user sees for a missing method is checked.
puts "spinel".upcase
