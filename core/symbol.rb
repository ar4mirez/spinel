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
    bare = /\A(?:[[:alpha:]_][[:alnum:]_]*[?!=]?|@@?[[:alpha:]_][[:alnum:]_]*|\$(?:[[:alpha:]_][[:alnum:]_]*|[0-9]+))\z/.match?(name) ||
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
