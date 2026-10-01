# Object — BasicObject plus Kernel, which is where almost everything lives.
#
# The `include Kernel` that makes that true is done at bootstrap, in Rust, so
# that `Integer.ancestors` is right from the first commit rather than from the
# moment this file loads.
#
# `to_s` and `inspect` are `Kernel`'s, as in CRuby —
# `Object.instance_method(:inspect).owner` is `Kernel` — so there is nothing to
# define here yet.
class Object
end

# `main`, the top level's `self`: an Object whose singleton answers `to_s` as
# "main" and forwards `include`, `define_method`, `public` and `private` to
# `Object`, privately — CRuby's set, measured. `public` and `private` are the
# primitives themselves, so a bare one sets the top-level frame's default.
class << self
  def to_s
    "main"
  end
  alias inspect to_s

  def include(*modules)
    Object.include(*modules)
  end

  def define_method(*args, &block)
    Object.__send__(:define_method, *args, &block)
  end

  alias public __main_public__
  alias private __main_private__

  private :include, :define_method, :public, :private
end
