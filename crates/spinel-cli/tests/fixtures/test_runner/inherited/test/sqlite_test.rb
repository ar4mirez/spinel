require_relative "base_test"

# An adapter whose roundtrip fails, through the test it inherits.
class SqliteTest < AdapterTest
  def value
    3
  end
end
