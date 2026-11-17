require "test_helper"
require_relative "../../app/models/account"
loaded(__FILE__)

class AccountTest < Minitest::Test
  def test_balance
    assert_equal 10, Account.balance
  end
end
