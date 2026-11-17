require "spec_helper"

RSpec.describe "noisy" do
  it("prints") { puts "an example prints" }

  it("fails with <markup> & more") do
    raise ArgumentError, "<i> & \"quoted\"\e[0m"
  end
end
