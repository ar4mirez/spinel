# `String#crypt` is libc's `crypt(3)`, which no slice has reached. Until one
# does, this file is how the message a user sees for a missing method is checked.
puts "spinel".crypt("ab")
