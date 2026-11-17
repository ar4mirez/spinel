require "test_helper"

class ResultsTest < Minitest::Test
  def test_passes
    assert_equal 2, 1 + 1
  end

  def test_fails
    assert_equal 3, 1 + 1
  end

  def test_errors
    raise ArgumentError, "broken"
  end

  def test_skips
    skip "not yet"
  end
end
