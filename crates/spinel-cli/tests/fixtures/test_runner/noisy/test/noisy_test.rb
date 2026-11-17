require "test_helper"

# Tests that print to stdout, and fail or skip with messages that XML
# must escape, or cannot hold as they are (an escape character).
class NoisyTest < Minitest::Test
  def test_prints
    puts "a test prints"
    $stdout.write("and writes\n")
    STDOUT.print("and prints to STDOUT\n")
  end

  def test_errors_with_markup
    raise ArgumentError, "<b> & \"quoted\" 'single'\e[31m\tred"
  end

  def test_skips_with_markup
    skip "<later> & \"then\""
  end
end
