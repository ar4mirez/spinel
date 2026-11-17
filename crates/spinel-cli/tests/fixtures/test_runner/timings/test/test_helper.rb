require "minitest/autorun"

# Each test file logs its name and the worker that loaded it, so the log
# shows which files each worker was handed.
def ran(file)
  File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("file #{Process.pid} #{File.basename(file)}\n") }
end
