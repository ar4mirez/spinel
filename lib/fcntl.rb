# `fcntl`: the constants for `IO#fcntl`, which are the platform's, from the
# same table `IO` reads its own from. CRuby's is a C extension that is
# nothing but these.

module Fcntl
  VERSION = "1.3.0"

  # `F_DUPFD_CLOEXEC` is in the table for `IO#dup`, not in CRuby's `Fcntl`.
  __fd_fcntl_constants__.each do |name, value|
    const_set(name, value) unless name == "F_DUPFD_CLOEXEC"
  end
end
