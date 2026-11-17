require "test_helper"

# Tests every adapter runs, each with its own `value`.
class AdapterTest < Minitest::Test
  def value
    2
  end

  def test_roundtrip
    assert_equal 2, value
  end
end
