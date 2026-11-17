# Never required while RSpec cannot run on Spinel (#835): a load is a line
# in the log, and `rspec` is not installed here.
File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec_helper #{Process.pid}\n") }
require "rspec"
