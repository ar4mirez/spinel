require "spec_helper"

RSpec.describe "results" do
  it("passes") { expect(1 + 1).to eq(2) }
  it("fails") { expect(1 + 1).to eq(3) }
  it("raises") { raise ArgumentError, "broken" }
  it("is pending") do
    pending "not yet"
    raise "still broken"
  end
  xit("is skipped") { raise "not run" }
  it("passes while pending") { pending "not yet" }
end
