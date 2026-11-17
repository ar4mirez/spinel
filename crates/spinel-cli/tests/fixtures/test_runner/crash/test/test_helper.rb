def ran(file)
  File.open(ENV.fetch("SPINEL_TEST_LOG"), "a") { |log| log.write("file #{Process.pid} #{File.basename(file)}\n") }
end
