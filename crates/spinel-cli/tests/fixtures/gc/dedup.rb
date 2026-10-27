# The fstring table holds a String `-@` made weakly (#384): once nothing else
# refers to it, `GC.start` lets it go, as a weak map's entry is let go. One a
# frozen literal answered is held by the literal from then on.
#
# Every String meant to die is made inside a method, so that once the method
# returns nothing but the map refers to it.

def dedup(map, text)
  string = -text.dup
  map[string] = string
  nil
end

def dedup_literal(map)
  string = -("li" + "t")
  map[string] = string
  string.equal?("lit".freeze)
end

map = ObjectSpace::WeakMap.new
kept = -("ke" + "pt")
map[kept] = kept
dedup(map, "gone")
p dedup_literal(map)
puts "before: #{map.size}"
GC.start
puts "after: #{map.size}"
p (-("ke" + "pt")).equal?(kept)
p map.values.map(&:to_s).sort
