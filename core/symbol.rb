# Symbol.
#
# `to_s`, `name`, `length` and `size` are primitives: they read the shared
# symbol table, which is the one table no heap owns.
class Symbol
  def to_sym
    self
  end

  # `:"a b"` and `:ab` are the same kind of object but do not print the same
  # way: a name that could not be written bare after a colon is quoted, and
  # `:"#{nil}"` is `:""` rather than a lone colon.
  #
  # Three shapes print bare, measured from CRuby 4.0.6 rather than recalled: an
  # operator method name, a plain identifier with an optional `?`, `!` or `=`,
  # and an ivar/cvar/gvar. The sigil forms take no suffix — `:a?` is bare but
  # `:"@a?"` is quoted — and a gvar is the one that may be all digits, so `:$1`
  # is bare where `:"@1"` is not.
  #
  # ponytail: the pattern and the operator list are rebuilt per call rather than
  # held in a constant, because a constant on `Symbol` would be visible to
  # `Symbol.constants` and Ruby has none there. `inspect` is not a hot path; if
  # it becomes one, the fix is a primitive, not a public constant.
  def inspect
    name = to_s
    # A name is bare when it could be written as a symbol literal: an
    # identifier (any non-ASCII character counts as a letter), an instance or
    # class variable, a global — including the one-character specials and `$-w`
    # style flags — or an operator. Measured.
    bare = /\A(?:[A-Za-z_\u0080-\u{10FFFF}][A-Za-z0-9_\u0080-\u{10FFFF}]*[?!=]?|@@?[A-Za-z_\u0080-\u{10FFFF}][A-Za-z0-9_\u0080-\u{10FFFF}]*|\$(?:[A-Za-z_\u0080-\u{10FFFF}][A-Za-z0-9_\u0080-\u{10FFFF}]*|[0-9]+|[~*$?!@\/\\;,.=:<>"&`'+0]|-[A-Za-z0-9_]))\z/.match?(name) ||
           ["+", "-", "*", "/", "%", "**", "==", "!=", ">", ">=", "<", "<=",
            "<=>", "===", "=~", "!~", "!", "~", "[]", "[]=", "<<", ">>", "&",
            "|", "^", "+@", "-@", "`"].include?(name)
    return ":" + name if bare
    ":" + name.inspect
  end

  def empty?
    length == 0
  end

  def <=>(other)
    return nil unless other.is_a?(Symbol)
    to_s <=> other.to_s
  end
end

# `Symbol#name` is one frozen String per symbol, the same object every call —
# measured — where `to_s` is a new, unfrozen one each time. The table is per
# heap, on `Symbol`.
class Symbol
  def name
    Symbol.__names__[self] ||= to_s.freeze
  end

  def self.__names__
    @__names__ ||= {}
  end
end
