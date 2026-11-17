require "spec_helper"

RSpec.describe "report" do
  it("passes") { expect(1 + 1).to eq(2) }

  it("fails") do
    expect(1 + 1).to eq(3)
  end

  it("is pending") do
    pending "not yet"
    raise "still broken"
  end
end
