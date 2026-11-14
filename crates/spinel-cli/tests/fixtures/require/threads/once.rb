# Eight threads require one slow file at once: it runs once, `require`
# answers true in the thread that ran it and false in the rest, and every
# thread's `require` returns only once the file has finished.
$LOAD_PATH.unshift File.join(__dir__, "lib")
$runs = 0
$finished = false
results = Queue.new
threads = 8.times.map do
  Thread.new { results << [require("slow_feature"), $finished] }
end
threads.each(&:join)
results = 8.times.map { results.pop }
p runs: $runs, true: results.count { |loaded, _| loaded }, all_after: results.all? { |_, after| after }
