require "shout"

module Greet
  VERSION = "2.0.0"

  def self.line
    Shout.up("greet #{VERSION}")
  end
end
