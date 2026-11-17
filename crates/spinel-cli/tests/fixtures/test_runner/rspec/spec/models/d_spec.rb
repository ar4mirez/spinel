File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec #{Process.pid} d_spec.rb #{HELPER_PID}\n") }
require "spec_helper"

RSpec.describe("d") { it("fails") { raise "a failing example fails the run" } }
