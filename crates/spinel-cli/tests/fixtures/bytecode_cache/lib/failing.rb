module Failing
  def self.call
    raise ArgumentError, "raised in a required file"
  end
end
