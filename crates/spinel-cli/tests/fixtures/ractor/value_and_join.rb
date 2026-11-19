r = Ractor.new(3, 4) { |x, y| x * y }
p r.value
j = Ractor.new { :done }
p j.join.equal?(j)
p j.value
