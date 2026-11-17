# `ripper` (#837), replaced by a Prism-backed shim (docs/engine.md, "Built-in
# gems"). CRuby's is `ext/ripper`, its parse.y run as an event-driven
# parser; Spinel parses with Prism, so `Ripper` is the prism gem's own
# translation, `Prism::Translation::Ripper` (`stdlib/prism/translation/`),
# which raises the same parser and scanner events over a Prism tree and its
# tokens. `prism/translation/ripper/shim` is upstream's way of putting it in
# the `Ripper` constant.
#
# What is Spinel's here is the part of `ext/ripper`'s C that reads the
# source, `ripper_initialize`: an object that answers `gets` is read a line
# at a time, and anything else is converted with `to_str`, so that
# `Ripper.sexp` and `Ripper.lex` take what CRuby's take — RSpec hands them
# an `RSpec::Support::EncodedString`. Prism's translation passes the object
# to `Prism.parse`, which takes only a String.
#
# ponytail: the translation is Prism 1.9.0's, with its gaps against
# parse.y, which Spinel reproduces byte for byte (Prism's translation on
# CRuby answers the same). On spec files `Ripper.sexp` and `Ripper.lex`
# answer what CRuby 4.0.9's do (`crates/spinel-cli/tests/ripper.rs`). Where
# they do not, measured: `lex` gives a different lexer state to some space
# and comment tokens (after a command's argument, an endless `def`'s
# parameters, an `in` pattern's keyword, a regexp literal) and to a
# heredoc's terminator; `sexp` leaves out the `args_add_block` around
# `return a = b`'s value, and ends a squiggly heredoc whose last line ends
# in an interpolation with an empty `@tstring_content` where parse.y has
# "\n"; `Ripper.name` is
# "Prism::Translation::Ripper"; and on a syntax error the recovery is
# Prism's. RSpec reads only tokens' kinds, text and positions and the
# tree's positions. Upgrade: fix them in Prism's translation upstream and
# take its next release.

require "prism"
require "prism/translation/ripper/shim"

class Ripper
  # `ripper_initialize`'s reading of `src`: lines from `gets` until nil, or
  # the String `to_str` makes of it.
  def self.__source__(src)
    return src if String === src
    if src.respond_to?(:gets)
      source = +""
      while (line = src.gets)
        raise TypeError, "gets returned #{line.class} (expected String or nil)" unless String === line
        source << line
      end
      return source
    end
    unless src.respond_to?(:to_str)
      name = case src
             when nil then "nil"
             when true then "true"
             when false then "false"
             else src.class
             end
      raise TypeError, "no implicit conversion of #{name} into String"
    end
    src.to_str
  end

  # Every parser `Ripper.sexp`, `Ripper.sexp_raw`, `Ripper.parse` and a
  # subclass's `new` make reads its source through `__source__`.
  module Source
    def initialize(src, filename = "(ripper)", lineno = 1)
      super(Ripper.__source__(src), filename, lineno)
    end
  end
  prepend Source

  # `Ripper.lex` and `Ripper.tokenize`, which lex without a parser object.
  module LexSource
    def lex(src, *rest, **options)
      super(Ripper.__source__(src), *rest, **options)
    end
  end
  singleton_class.prepend LexSource
  private_constant :Source, :LexSource
end
