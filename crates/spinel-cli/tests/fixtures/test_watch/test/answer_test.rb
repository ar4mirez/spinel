require "test_helper"
require_relative "../config/answer"
loaded(__FILE__)

# Fails until config/answer.rb says 2.
class AnswerTest < Minitest::Test
  def test_answer
    assert_equal 2, ANSWER
  end
end
