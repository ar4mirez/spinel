# Tests a test class includes, defined in a file that is not a test file.
module SharedTests
  def test_shared_fails
    assert_equal 2, 1 + 2
  end
end
