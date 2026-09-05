# Object — BasicObject plus Kernel, which is where almost everything lives.
#
# The `include Kernel` that makes that true is done at bootstrap, in Rust, so
# that `Integer.ancestors` is right from the first commit rather than from the
# moment this file loads.
class Object
  # `self.class.to_s`, not `.name`: an instance of an anonymous class has a
  # class whose `name` is nil, and `"#<" + nil` is a TypeError.
  #
  # ponytail: Ruby puts the address in here too — `#<Foo:0x...>`. #15 left that
  # out rather than invent one, and this slice does not change it.
  def to_s
    "#<" + self.class.to_s + ">"
  end

  def inspect
    to_s
  end
end
