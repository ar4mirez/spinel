# `spinel run --watch` restarts this program when the project changes
# (tests/watch.rs). Each run prints its pid, and the pid of a child it forks,
# so the test can see the old processes are gone after a restart.
$stdout.sync = true
require_relative "config/settings"

if ARGV.first == "once"
  puts "started #{$$}"
  exit 3
end

grandchild = fork { sleep }
puts "started #{$$} #{grandchild}"
sleep
