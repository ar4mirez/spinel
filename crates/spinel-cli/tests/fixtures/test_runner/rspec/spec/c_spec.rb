File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec #{Process.pid} c_spec.rb #{HELPER_PID}\n") }
require "spec_helper"

RSpec.describe("c") { it("passes") { true } }
