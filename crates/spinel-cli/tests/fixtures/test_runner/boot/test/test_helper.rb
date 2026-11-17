# Required once, by the parent, before any worker is forked. Each load is a
# line in the log, so a worker that loaded it again would add one.
LOG = ENV.fetch("SPINEL_TEST_LOG")
File.open(LOG, "a") { |log| log.write("test_helper #{Process.pid}\n") }
BOOTED_BY = Process.pid

require "greeting"

# Log the file and the worker running it, then wait until as many workers as
# `SPINEL_TEST_BARRIER` asks for have each logged a file: only that many
# workers running at once gets every file past here.
def ran(file)
  File.open(LOG, "a") do |log|
    log.write("file #{Process.pid} #{BOOTED_BY} #{Greeting.hello} #{File.basename(file)}\n")
  end
  want = Integer(ENV.fetch("SPINEL_TEST_BARRIER", "1"))
  deadline = Time.now + 30
  loop do
    workers = File.readlines(LOG).grep(/\Afile /).map { |line| line.split[1] }.uniq
    break if workers.size >= want
    raise "#{workers.size} of #{want} workers ran at once" if Time.now > deadline

    sleep 0.02
  end
end
