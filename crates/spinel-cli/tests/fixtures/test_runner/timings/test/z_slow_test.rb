require "test_helper"

ran(__FILE__)

# The slow file, last in file order.
class SlowTest < Minitest::Test
  def test_slow
    sleep 2.0
    assert_equal 2, 1 + 1
  end
end
