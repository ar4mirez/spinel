# A child forked from a thread other than the main one makes that thread its
# main thread: it handles a trapped signal, and an untrapped SIGTERM ends it
# (#44).
trap("USR1") { puts "trapped"; $stdout.flush }
child = Thread.new do
  fork do
    puts Thread.current.equal?(Thread.main)
    $stdout.flush
    Process.kill(:USR1, Process.pid)
    trap("USR1", "DEFAULT")
    spawn("sh", "-c", "sleep 0.1; kill -TERM #{Process.pid}")
    sleep 5
    puts "not ended"
    exit!(1)
  end
end.value
Process.wait(child)
p $?.termsig
