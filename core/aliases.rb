# Aliases that are aliases (#27).
#
# Ruby defines `collect` as an alias of `map`, not as a second method that
# calls it, and now that a method can be taken out of its class and compared
# the difference shows: `Array.instance_method(:collect) ==
# Array.instance_method(:map)` is true in Ruby, and ruby/spec checks each pair.
# The core library wrote many of them as two definitions. This file, loaded
# last, makes each the alias it is in CRuby.
#
# Which name is the alias is CRuby's choice, except where the definition
# here was written the other way round — `Hash#include?` calls `key?` — and
# aliasing it as CRuby does would point each at the other. Those are aliased
# from the name that has the body; the two are one method either way.
#
# ponytail: the second definitions are still in their files, shadowed. Each
# should become an `alias` where it is written, and this file should shrink
# to nothing.

class Array
  alias collect map
  alias collect! map!
  alias detect find
  alias filter select
  alias filter! select!
  alias prepend unshift
end

class BasicObject
  alias equal? ==
end

class Dir
  class << self
    alias getwd pwd
    alias delete rmdir
    alias unlink rmdir
  end
end

class Encoding
  alias to_s name
end

module Enumerable
  alias collect_concat flat_map
  alias collect map
  alias detect find
  alias entries to_a
  alias filter select
  alias find_all select
  alias member? include?
  alias reduce inject
end

class Enumerator::Lazy
  alias collect_concat flat_map
  alias collect map
  alias enum_for to_enum
  alias filter select
  alias find_all select
end

class Enumerator
  alias each_with_object with_object
end

class FalseClass
  alias ^ |
  alias inspect to_s
end

class File
  class << self
    alias unlink delete
    alias empty? zero?
  end
end

class Float
  alias inspect to_s
end

class Hash
  alias each each_pair
  alias filter select
  alias filter! select!
  alias include? key?
  alias has_key? key?
  alias length size
  alias member? key?
  alias store []=
  alias update merge!
end

class Integer
  alias inspect to_s
  alias modulo %
  alias next succ
end

module Kernel
  alias enum_for to_enum
  alias format sprintf
  alias kind_of? is_a?
  alias then yield_self

  class << self
    alias format sprintf
  end
end

class MatchData
  alias eql? ==
end

class Module
  alias inspect to_s
end

class NilClass
  alias ^ |
end

class Proc
  alias inspect to_s
end

class Range
  alias entries to_a
  alias member? include?
end

class Regexp
  alias eql? ==
end

class String
  alias next! succ!
  alias slice []
  alias to_str to_s
end

class Thread
  class << self
    alias fork start
  end
end

class TrueClass
  alias inspect to_s
end
