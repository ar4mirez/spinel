# Never required: `rspec-core` is not installed here, so `spinel test` stops
# at `require "rspec/core"`. A load is a line in the log.
File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec_helper #{Process.pid}\n") }
