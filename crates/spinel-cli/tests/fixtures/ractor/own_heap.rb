class Thing; end
things = Array.new(500) { Thing.new }
p ObjectSpace.each_object(Thing).count >= 500
r = Ractor.new do
  [ObjectSpace.each_object(Thing).count, GC.stat(:total_allocated_objects) < 100_000]
end
p r.value
p things.size
