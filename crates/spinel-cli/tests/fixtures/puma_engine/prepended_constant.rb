# `A::X` reads `A`'s own table before a module prepended to `A`, which moves
# `A`'s methods behind it and not its constants: minitest-retry prepends to
# `Minitest`, and `Minitest::VERSION` stays minitest's.
module R
  V = :prepended
  W = :only_in_r
end

module M
  V = :own
  prepend R
end

class C
  V = :own
  prepend R
end

p M::V, C::V, M::W, C::W, M.const_get(:V), C.const_get(:V), defined?(M::V)
module M
  p V
end
class C
  p V
end
