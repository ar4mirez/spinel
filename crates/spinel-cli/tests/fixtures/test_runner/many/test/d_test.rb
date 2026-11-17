require "test_helper"

class ManyDTest < Minitest::Test
  include Many

  MANY.times { |index| define_method("test_#{index}") { outcome(index) } }
end
