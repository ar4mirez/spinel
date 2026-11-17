File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec #{Process.pid} b_spec.rb #{HELPER_PID}\n") }
require "spec_helper"

RSpec.describe("b") { it("passes") { true } }
