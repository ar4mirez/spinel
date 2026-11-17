# Stands for an installed gem that calls back into the test, so a failure
# has the gem's frames in the middle of its backtrace.
module Checker
  def self.check(value, &block)
    first(value, &block)
  end

  def self.first(value, &block)
    second(value, &block)
  end

  def self.second(value)
    yield value
  end
end
