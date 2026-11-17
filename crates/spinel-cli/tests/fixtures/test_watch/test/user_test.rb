require "test_helper"
require "user"
loaded(__FILE__)

class UserTest < Minitest::Test
  def test_name
    assert_equal "ada", User.name
  end
end
