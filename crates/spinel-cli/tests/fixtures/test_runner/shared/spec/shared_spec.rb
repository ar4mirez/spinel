require "spec_helper"

RSpec.describe "shared" do
  it("passes") { expect(1 + 1).to eq(2) }

  it_behaves_like "a sum"
end
