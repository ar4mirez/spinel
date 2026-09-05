# The definition of done for #15: this file, run by the built binary.
#
# Every line exercises something `core/*.rb` defines on top of a primitive, so a
# regression anywhere in the core library shows up as a diff in one output.
puts "hello"

numbers = []
numbers << 1
numbers << 2
numbers.push(3)
puts numbers.inspect
puts numbers.map { |n| n * 2 }.inspect
puts numbers.select { |n| n.odd? }.inspect
puts numbers.sum
puts numbers.include?(2)
puts numbers.first
puts numbers.last

words = ["a", "b", "c"]
puts words.join("-")
puts words.reverse.inspect

counts = Hash.new(0)
counts["x"] = counts["x"] + 1
counts["y"] = 7
puts counts.inspect
puts counts.keys.inspect
puts counts["never set"]

puts 255.to_s(16)
puts (2 ** 10)
puts 10.times { |i| } .inspect rescue puts "times answers self"
puts "spinel".length
puts "spinel"[1, 3]
puts :symbol.inspect
puts 1.5.to_s
puts nil.inspect
puts [1, [2, 3], "four", :five, nil].inspect
