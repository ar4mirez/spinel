require "test_helper"

# One test class split across two files: this one defines it.
class SplitTest < Minitest::Test
  def test_a
    assert_equal 2, 1 + 1
  end
end
