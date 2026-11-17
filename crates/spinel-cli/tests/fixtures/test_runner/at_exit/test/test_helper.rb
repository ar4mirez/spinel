# The `at_exit` a framework registers from its helper, as `minitest/autorun`
# does: it runs in each worker, after its last file, and not in the parent.
# `SPINEL_TEST_AT_EXIT` makes it end the worker as failing tests or a crash
# while running them would.
LOG = ENV.fetch("SPINEL_TEST_LOG")
at_exit do
  File.open(LOG, "a") { |log| log.write("at_exit #{Process.pid}\n") }
  case ENV["SPINEL_TEST_AT_EXIT"]
  when "fail" then exit 1
  when "kill" then Process.kill(:KILL, Process.pid)
  end
end

def ran(file)
  File.open(LOG, "a") { |log| log.write("file #{Process.pid} #{File.basename(file)}\n") }
end
