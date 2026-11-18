# The gem's Ruby half, which loads its native half and builds on it, as a
# gem with a C extension does.
require "native_gem/version"
require "native_gem/native_gem"

module NativeGem
  # `text` followed by itself reversed, over the native `reverse`.
  def self.mirror(text)
    text + reverse(text)
  end
end
