File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec_helper #{Process.pid}\n") }
require_relative "../test/test_helper"
