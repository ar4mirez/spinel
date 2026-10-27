# A Warning.warn whose category: is optional gets it, from Kernel#warn and
# from a chilled String's deprecation; one taking only the message gets one
# argument (#374).
module Warning; def self.warn(m, category: nil) = puts("W #{category.inspect}"); end
warn("y", category: :deprecated)
"a" << "b"
module Warning; def self.warn(m) = puts("1 #{m.class}"); end
warn("y", category: :deprecated)
"c" << "d"
