File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec #{Process.pid} c_spec.rb\n") }
require "spec_helper"

RSpec.describe("c") { it("fails") { raise "never a silent green" } }
