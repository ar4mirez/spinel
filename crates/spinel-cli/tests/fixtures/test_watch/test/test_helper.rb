require "minitest/autorun"

# Each test file loaded is a line in the log, with the pid that loaded it.
def loaded(file)
  File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("#{File.basename(file)} #{Process.pid}\n") }
end
