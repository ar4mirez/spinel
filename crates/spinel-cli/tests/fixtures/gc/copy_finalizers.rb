# `dup` and `clone` give the copy the original's finalizers, as CRuby's
# `init_copy` does (#36): each runs once for the copy, after the copy is
# collected, and once more for the original. Every copy meant to die is made
# inside a method, so that once it returns nothing refers to it.

def recorder(seen, name)
  ->(_id) { seen << name }
end

def copies(object)
  object.dup
  object.clone
  nil
end

seen = []
object = Object.new
ObjectSpace.define_finalizer(object, recorder(seen, :object))
copies(object)
GC.start
p seen

# A String that is only a String is copied by `dup` without them, and by
# `clone` with them. Measured on CRuby 4.0.9.
seen = []
string = +"s"
ObjectSpace.define_finalizer(string, recorder(seen, :string))
copies(string)
GC.start
p seen

# A copy's finalizers are its own: undefining them leaves the original's.
def undefined_copy(object)
  ObjectSpace.undefine_finalizer(object.dup)
  nil
end

seen = []
undefined_copy(object)
GC.start
p seen

# An IO's copy has a descriptor of its own, and a finalizer for that one
# only: collecting the copy leaves the original open. A program's own
# finalizer still comes along.
seen = []
reader, writer = IO.pipe
ObjectSpace.define_finalizer(writer, recorder(seen, :writer))
def io_copy(io) = (io.dup; nil)
io_copy(writer)
GC.start
writer.write "still open"
writer.close
p reader.read, seen
