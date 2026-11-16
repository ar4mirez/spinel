require "rbconfig"

module Respawn
  # What a child started as the current Ruby reports: its engine and what it
  # takes to be the current Ruby in turn.
  CHILD = 'print RUBY_ENGINE, " ", Gem.ruby'

  # Start the current Ruby the two ways gems do: `Gem.ruby` (RubyGems' own
  # hooks, Rake's `ruby`) and `RbConfig.ruby` (WEBrick, Bundler's `exec`).
  def self.children
    {
      "gem_ruby" => IO.popen([Gem.ruby, "-e", CHILD], &:read),
      "rbconfig_ruby" => IO.popen([RbConfig.ruby, "-e", CHILD], &:read),
    }
  end
end
