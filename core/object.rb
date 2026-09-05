# Object — BasicObject plus Kernel, which is where almost everything lives.
#
# The `include Kernel` that makes that true is done at bootstrap, in Rust, so
# that `Integer.ancestors` is right from the first commit rather than from the
# moment this file loads.
class Object
  def to_s
    "#<" + self.class.name + ">"
  end

  def inspect
    to_s
  end
end
