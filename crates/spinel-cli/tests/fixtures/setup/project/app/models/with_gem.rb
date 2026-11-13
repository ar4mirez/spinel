# The shim, given a `Gem` that is already loaded, tells it the gem home.
# ARGV[0] is the shim; this stands in for RubyGems until it loads at boot.
module Gem
  def self.paths=(env)
    @told = env
  end

  def self.told
    @told
  end
end

load ARGV[0]
p Gem.told
