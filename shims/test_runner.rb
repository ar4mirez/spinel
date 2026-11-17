# Spinel's test runner (docs/test-runner.md, "Execution"): what `spinel test`
# runs on the VM, after `shims/setup.rb` and the gem prelude, as `spinel run`
# runs a program. `ARGV` is the worker count the CLI chose (`-j`, else the
# physical cores); it is cleared before any test code can read it.
#
# 1. Discovery: `test/**/*_test.rb` (Minitest) and `spec/**/*_spec.rb`
#    (RSpec), relative to the current directory, each sorted.
#    ponytail: RSpec files are found but not run, because RSpec cannot run
#    its examples on the VM yet (#835, splatting an object whose `to_a` is
#    Ruby). `spec/` is not put on `$LOAD_PATH`, `spec/spec_helper.rb` is not
#    required, no spec file goes to a worker, and the run says so on stderr
#    and exits 1, so a spec suite is never a silent green. Once #835 lands,
#    the helper is required with the others and each worker runs the
#    examples of the spec files it loaded through RSpec's own runner.
# 2. `lib/` and `test/` go on `$LOAD_PATH`, as Rake's TestTask puts them, and
#    `test/test_helper.rb` is required here, once, before any worker exists:
#    whatever it boots, every worker inherits through `fork` instead of
#    booting again.
# 3. Workers: `fork` after the helpers, up to the worker count, never more
#    than there are files. Each worker has two pipes to this process: one
#    it reads paths from, one line each, and one it writes events to. A path
#    is handed to a worker only when it has finished the one before, so the
#    files are shared out one at a time and a slow file holds up only its
#    own worker. A worker requires each file it is given, as Rake's loader
#    requires them, and after its last file ends as a program ends, so the
#    `at_exit` a framework registered (`minitest/autorun`) runs its tests.
# 4. Results: a worker writes one JSON line per event, for now only
#    `{"event":"done","file":..,"error":..}` once a file is loaded, `error`
#    the exception that loading raised or `nil`. #93's reporter shim replaces
#    this with one line per test (`pass|fail|error|skip`) on the same pipe,
#    and #94's aggregator replaces the reporting below.
# 5. A worker that ends before it says `done` for the file it was given —
#    killed by a signal, or exiting, with any status — is reported with that
#    file, and a new worker is forked for the files still waiting, so one
#    crash costs one file, not the rest of the suite. A worker that ends by a
#    signal or a failing status after its last file is reported too, with
#    the files it ran, and so is a file whose loading raised. Any of these,
#    or a spec file found, makes the exit status 1.
#
# The runner ends with `exit!`, so an `at_exit` a helper registered runs in
# the workers, which ran the tests, and not again here with none loaded.

require "json"

module Spinel
  module TestRunner
    Worker = Struct.new(:number, :pid, :tasks, :events, :buffer, :file, :ran)

    # How the end of a process reads: a signal by its name, as `Process::Status`
    # names it, else the exit status.
    def self.ending(status)
      if status.signaled?
        name = Signal.signame(status.termsig)
        name ? "was killed by SIG#{name}" : "was killed by signal #{status.termsig}"
      else
        "exited with status #{status.exitstatus}"
      end
    end

    def self.run(jobs)
      files = Dir.glob("test/**/*_test.rb").sort
      specs = Dir.glob("spec/**/*_spec.rb").sort
      if files.empty? && specs.empty?
        warn "spinel test: no test files: nothing matches test/**/*_test.rb or spec/**/*_spec.rb"
        return 1
      end
      failed = run_tests(files, jobs) unless files.empty?
      unless specs.empty?
        failed = true
        warn "spinel test: #{specs.size} RSpec file#{"s" unless specs.size == 1} under spec/ not run: " \
             "RSpec suites cannot run on Spinel yet (https://github.com/ar4mirez/spinel/issues/835)"
      end
      failed ? 1 : 0
    end

    # Run `files` on up to `jobs` workers; true when the suite failed.
    def self.run_tests(files, jobs)
      %w[test lib].each do |dir|
        path = File.expand_path(dir)
        $LOAD_PATH.unshift(path) if File.directory?(path) && !$LOAD_PATH.include?(path)
      end
      require File.expand_path("test/test_helper.rb") if File.file?("test/test_helper.rb")

      queue = files.dup
      workers = {}
      failed = false
      count = 0
      add_worker = lambda do
        count += 1
        worker = start(count, workers)
        workers[worker.events] = worker
        hand_out(worker, queue)
      end
      [jobs, files.size].min.times { add_worker.call }

      until workers.empty?
        ready, = IO.select(workers.keys)
        ready.each do |io|
          worker = workers[io]
          chunk = begin
            io.read_nonblock(65_536)
          rescue IO::WaitReadable
            next
          rescue EOFError
            nil
          end
          if chunk
            worker.buffer << chunk
            while (newline = worker.buffer.index("\n"))
              line = worker.buffer.slice!(0..newline)
              failed = true unless event(worker, JSON.parse(line), queue)
            end
            next
          end

          # Its end of the events pipe is closed: the worker has ended.
          workers.delete(io)
          io.close
          worker.tasks.close unless worker.tasks.closed?
          _, status = Process.wait2(worker.pid)
          if worker.file
            failed = true
            warn "spinel test: worker #{worker.number} (pid #{worker.pid}) #{ending(status)} while running #{worker.file}"
            add_worker.call unless queue.empty?
          elsif !status.success?
            # Failed tests (`minitest/autorun` exits 1), or a crash while
            # the `at_exit` ran them.
            failed = true
            ran = worker.ran.empty? ? "no files" : worker.ran.join(", ")
            warn "spinel test: worker #{worker.number} (pid #{worker.pid}) #{ending(status)} after its last file; it ran #{ran}"
          end
        end
      end
      failed
    end

    # Fork worker `number`. The child closes the parent's ends of every
    # other worker's pipes, so each pipe's end is seen when its own worker
    # ends and not when the last worker forked after it does.
    def self.start(number, workers)
      tasks_r, tasks_w = IO.pipe
      events_r, events_w = IO.pipe
      pid = fork do
        workers.each_value do |other|
          other.tasks.close unless other.tasks.closed?
          other.events.close
        end
        tasks_w.close
        events_r.close
        work(tasks_r, events_w)
      end
      tasks_r.close
      events_w.close
      tasks_w.sync = true
      Worker.new(number, pid, tasks_w, events_r, +"", nil, [])
    end

    # The worker: each path read is a file to require, then a `done` event.
    # An event is one `write` on a `sync` pipe, so a line never straddles two
    # flushes.
    def self.work(tasks, events)
      events.sync = true
      while (line = tasks.gets)
        file = line.chomp
        error = nil
        begin
          require File.expand_path(file)
        rescue StandardError, ScriptError => e
          error = "#{e.class}: #{e.message}"
        end
        events.write(JSON.generate({ "event" => "done", "file" => file, "error" => error }) + "\n")
      end
    end

    # One event from `worker`; false when it says the suite failed.
    def self.event(worker, event, queue)
      return true unless event["event"] == "done"

      worker.ran << worker.file
      worker.file = nil
      hand_out(worker, queue)
      return true unless event["error"]

      warn "spinel test: #{event["file"]}: #{event["error"]}"
      false
    end

    # The next waiting file for `worker`, or, with none left, the end of its
    # tasks pipe, which ends it. A worker that has already ended cannot take
    # it; the end of its events pipe reports it with the file.
    def self.hand_out(worker, queue)
      if (file = queue.shift)
        worker.file = file
        begin
          worker.tasks.write("#{file}\n")
        rescue Errno::EPIPE
          nil
        end
      else
        worker.tasks.close
      end
    end
  end
end

jobs = Integer(ARGV.shift)
ARGV.clear
code = Spinel::TestRunner.run(jobs)
$stdout.flush
$stderr.flush
exit!(code)
