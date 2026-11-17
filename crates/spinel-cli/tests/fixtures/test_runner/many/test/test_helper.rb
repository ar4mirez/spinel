require "minitest/autorun"

# Each file defines this many tests; every tenth skips with a message long
# enough that its event line is longer than a pipe holds and than the
# runner reads at once, in characters JSON escapes and in several bytes
# each.
MANY = 100
LONG = ("é\"\\\n\t" * 20_000).freeze

module Many
  def outcome(index)
    skip(LONG) if (index % 10).zero?
    assert_equal index, index
  end
end
