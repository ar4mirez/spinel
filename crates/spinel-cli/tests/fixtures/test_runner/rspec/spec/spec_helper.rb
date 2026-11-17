# Required once, in the RSpec phase's process, before the workers are forked: a load is a
# line in the log with the pid that did it, and what it defines is seen by
# the spec files as `HELPER_PID`.
File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("spec_helper #{Process.pid}\n") }
HELPER_PID = Process.pid
