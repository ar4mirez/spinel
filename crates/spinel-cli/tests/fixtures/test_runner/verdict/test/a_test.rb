require "test_helper"

class VerdictTest < Minitest::Test
  # With `SPINEL_TEST_GARBAGE`, a line on the events pipe that is not an
  # event, as a test writing to a descriptor it inherited could leave.
  def test_verdict
    if ENV["SPINEL_TEST_GARBAGE"]
      Spinel::TestWorker.events.write("not an event\n")
    else
      assert_equal 3, 1 + 1
    end
  end
end
