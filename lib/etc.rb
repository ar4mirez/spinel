# `etc`, as far as WEBrick needs it (#54): something `require "etc"` loads,
# because `webrick/utils.rb` requires it at the top. Only `WEBrick::Utils.su`
# calls into it, and only when a server is told to drop privileges.
#
# ponytail: the module and its version, none of its methods. CRuby's is a C
# extension over getpwnam(3), sysconf(3), uname(2) and the rest, so those are
# primitives to write; #111 ships them and makes `library/etc` pass. Until
# then a call is a NoMethodError, which says which method is missing.
module Etc
  VERSION = "1.4.6"
end
