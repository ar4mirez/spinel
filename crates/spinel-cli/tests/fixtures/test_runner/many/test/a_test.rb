require "test_helper"

class ManyATest < Minitest::Test
  include Many

  MANY.times { |index| define_method("test_#{index}") { outcome(index) } }
end
