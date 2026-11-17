require "test_helper"

# And this one reopens it.
class SplitTest < Minitest::Test
  def test_b
    assert_equal 3, 1 + 1
  end
end
