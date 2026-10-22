# Symbol.
#
# `to_s`, `name`, `length` and `size` are primitives: they read the shared
# symbol table, which is the one table no heap owns.
class Symbol
  def to_sym
    self
  end

  # Matching is the name's: the Symbol is its String for the purpose.
  def =~(pattern)
    to_s =~ pattern
  end

  def match(*args, &block)
    to_s.match(*args, &block)
  end

  def match?(*args)
    to_s.match?(*args)
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
    # And only a name the inspect encoding can show is bare: CRuby's
    # `rb_str_symname_p`.
    return ":" + name if bare && (name.ascii_only? || name.encoding == String.__inspect_encoding__)
    ":" + name.inspect
  end

  def empty?
    length == 0
  end

  def <=>(other)
    return nil unless other.is_a?(Symbol)
    to_s <=> other.to_s
  end

  # Identity: one name is one Symbol. Ruby defines it on Symbol rather than
  # leaving it to Comparable, so `===` can be its alias. The VM compares two
  # symbols without sending this; it is here for `send` and reflection.
  def ==(other)
    equal?(other)
  end
  alias === ==

  alias intern to_sym

  # The rest read the name as a String, which is what Ruby's own definitions
  # do (`rb_sym2str` then the String method): indexing answers a String, the
  # case methods and `succ` answer the Symbol of the String they answer.
  def [](*args)
    to_s[*args]
  end
  alias slice []

  def start_with?(*prefixes)
    to_s.start_with?(*prefixes)
  end

  def end_with?(*suffixes)
    to_s.end_with?(*suffixes)
  end

  def encoding
    to_s.encoding
  end

  def capitalize(*options)
    to_s.capitalize(*options).to_sym
  end

  def downcase(*options)
    to_s.downcase(*options).to_sym
  end

  def upcase(*options)
    to_s.upcase(*options).to_sym
  end

  def swapcase(*options)
    to_s.swapcase(*options).to_sym
  end

  def succ
    to_s.succ.to_sym
  end
  alias next succ

  # A Symbol against anything else is nil, not a TypeError: `String#casecmp`
  # would convert a String argument, and `:a.casecmp("a")` is nil.
  def casecmp(other)
    return nil unless other.is_a?(Symbol)
    to_s.casecmp(other.to_s)
  end

  def casecmp?(other)
    return nil unless other.is_a?(Symbol)
    to_s.casecmp?(other.to_s)
  end

  # Every Symbol is interned in the one shared table, which this reads.
  def self.all_symbols
    __all_symbols__
  end

  # A Symbol exists only by being interned: `Symbol.allocate` raises and
  # `Symbol.new` is not there to call.
  def self.allocate
    raise TypeError, "allocator undefined for Symbol"
  end

  class << self
    undef_method :new
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
