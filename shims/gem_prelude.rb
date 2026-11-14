# Spinel's gem prelude (docs/cli.md, "What every program starts with").
#
# CRuby's `gem_prelude.rb`, which every `ruby` runs before the `-r` files and
# the program. The CLI defines `Gem` and `DidYouMean` as empty modules first
# for the features the command line leaves on, as `ruby.c` does, and this
# loads what each one stands for: `--disable-gems` leaves both undefined,
# `--disable=did_you_mean` the second.
#
# ponytail: CRuby's prelude also loads `error_highlight` and
# `syntax_suggest/core_ext`; Spinel does not yet, so `--enable` naming them
# changes nothing. Each wants its own slice: error_highlight reads the code
# at a backtrace location, which needs `RubyVM::AbstractSyntaxTree`.

begin
  require "rubygems"
rescue LoadError => e
  raise unless e.path == "rubygems"

  warn "`RubyGems' were not loaded."
else
  require "bundled_gems"
end if defined?(Gem)

begin
  require "did_you_mean"
rescue LoadError
  warn "`did_you_mean' was not loaded."
end if defined?(DidYouMean)
