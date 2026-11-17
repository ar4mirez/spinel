# Examples a group includes, defined in a file that is not a spec file.
RSpec.shared_examples "a sum" do
  it("adds up") { expect(1 + 2).to eq(2) }
end
