require "test_helper"

ran(__FILE__)

class BTest < Minitest::Test
  def test_quick
    sleep 0.25
    assert_equal 2, 1 + 1
  end
end
