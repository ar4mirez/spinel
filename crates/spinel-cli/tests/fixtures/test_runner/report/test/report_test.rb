require "test_helper"

class ReportTest < Minitest::Test
  def test_passes
    assert_equal 2, 1 + 1
  end

  def test_fails
    assert_equal 3, 1 + 1
  end

  def test_fails_through_a_gem
    Checker.check(2) do |value|
      assert_equal 4, value
    end
  end

  def test_errors
    raise ArgumentError, "broken"
  end

  def test_skips
    skip "not yet"
  end
end
