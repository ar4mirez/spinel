require "test_helper"
require "support/shared_tests"

class SharedTest < Minitest::Test
  include SharedTests

  def test_passes
    assert_equal 2, 1 + 1
  end
end
