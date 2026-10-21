# Weak references clear on collection, and finalizers run after it — handed
# the object's id, never the object (#36).
#
# Every object meant to die is made inside a method, so that once the method
# returns nothing but the maps refers to it.

def fill(map, keys)
  3.times { map[Object.new] = Object.new }
  3.times { keys[Object.new] = Object.new }
end

def keep(map, keys, kept, key)
  map[kept] = kept
  keys[key] = Object.new
end

map = ObjectSpace::WeakMap.new
keys = ObjectSpace::WeakKeyMap.new
kept = Object.new
key = Object.new
keep(map, keys, kept, key)
fill(map, keys)
puts "before: #{map.size} #{keys.inspect[/size=\d+/]}"
GC.start
# The entries whose halves nothing else held are gone. A WeakKeyMap value is
# held by its key, so the one whose key is alive kept its value.
puts "after: #{map.size} #{keys.inspect[/size=\d+/]}"
p map[kept].equal?(kept)
p keys[key].class

# Made away from the object, so the block's environment does not hold it.
def recorder(map, seen)
  ->(id) { seen << [id, map.size] }
end

def doomed(map, seen)
  object = Object.new
  map[object] = :doomed
  ObjectSpace.define_finalizer(object, recorder(map, seen))
  object.object_id
end

seen = []
map = ObjectSpace::WeakMap.new
id = doomed(map, seen)
GC.start
# The finalizer ran once the object was collected: it was handed the id, and
# by then the object was gone from the weak map too.
p seen == [[id, 0]]

at_exit { puts "at_exit" }
ObjectSpace.define_finalizer(Object.new, proc { puts "finalizer at exit" })
