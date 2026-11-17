# Logs its own load, each test file run, and its `at_exit`, as
# `minitest/autorun` registers one, with the pid that did it.
LOG = ENV.fetch("SPINEL_TEST_LOG")
File.open(LOG, "a") { |log| log.write("test_helper #{Process.pid}\n") }
at_exit { File.open(LOG, "a") { |log| log.write("at_exit #{Process.pid}\n") } }

def ran(file)
  File.open(LOG, "a") { |log| log.write("file #{Process.pid} #{File.basename(file)}\n") }
end
