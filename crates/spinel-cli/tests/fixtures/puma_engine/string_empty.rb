# `String#empty?` is whether the String has no bytes: it counts no
# characters (Puma's MiniSSL asks it of each slice of a body it writes) and
# does not ask `length`.
class Counted < String
  def length = 5
end

p "".empty?, "a".empty?, Counted.new.empty?, Counted.new("x").empty?
broken = ("€" * 4).byteslice(1..)
p broken.valid_encoding?, broken.empty?, broken.byteslice(0, 0).empty?
