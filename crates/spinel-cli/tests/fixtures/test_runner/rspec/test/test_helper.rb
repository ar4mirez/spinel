# Logs its own load and each test file run, with the pid that did it.
LOG = ENV.fetch("SPINEL_TEST_LOG")
File.open(LOG, "a") { |log| log.write("test_helper #{Process.pid}\n") }

def ran(file)
  File.open(LOG, "a") { |log| log.write("file #{Process.pid} #{File.basename(file)}\n") }
end
