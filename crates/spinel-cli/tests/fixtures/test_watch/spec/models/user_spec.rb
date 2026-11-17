require "spec_helper"
require "user"

RSpec.describe("User") { it("is named") { expect(User.name).to eq("ada") } }
