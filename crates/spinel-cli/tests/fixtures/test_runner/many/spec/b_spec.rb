require "spec_helper"

RSpec.describe "b" do
  MANY.times do |index|
    it(index.to_s) do
      pending(LONG) if (index % 10).zero?
      raise "expected" if (index % 10).zero?
    end
  end
end
