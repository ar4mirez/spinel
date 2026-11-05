# Dumping a singleton class reads it without sending it anything, so no
# singleton class of a singleton class is made while `dump_all` walks the
# heap (#690). `dump_all` lists those too, so one made by a walk would show
# as one more class in the next.
require 'objspace'

def classes_dumped
  ObjectSpace.dump_all(output: :string).lines.count { |line| line.include?('"type":"CLASS"') }
end

object = Object.new
def object.greet; end
singleton = object.singleton_class
ObjectSpace.dump(singleton)
counts = Array.new(3) { classes_dumped }
p counts.uniq.size
