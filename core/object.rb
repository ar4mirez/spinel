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
