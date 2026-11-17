# Spinel's test runner (docs/test-runner.md, "Execution"): what `spinel test`
# runs on the VM, after `shims/setup.rb` and the gem prelude, as `spinel run`
# runs a program. `ARGV` is the worker count the CLI chose (`-j`, else the
# physical cores); it is cleared before any test code can read it.
#
# 1. Discovery: `test/**/*_test.rb` (Minitest) and `spec/**/*_spec.rb`
#    (RSpec), relative to the current directory, each sorted. The Minitest
#    files run first, then the RSpec files. Each phase runs in a process of
#    its own forked from this one, the RSpec one only after the Minitest one
#    has ended, so neither framework's helper or `at_exit` reaches the
#    other's workers.
# 2. Minitest: the phase process puts `lib/` and `test/` on `$LOAD_PATH`, as
#    Rake's TestTask puts them, and requires `test/test_helper.rb` once,
#    before any worker exists: whatever it boots, every worker inherits
#    through `fork` instead of booting again. RSpec (#837): the phase
#    process puts `lib/` and `spec/` on `$LOAD_PATH`, as the `rspec` command
#    puts them, and requires `rspec/core` and `spec/spec_helper.rb` once,
#    the same way.
# 3. Workers: each phase process forks its workers after the helpers, up to
#    the worker count, never more than there are files. Each worker has two
#    pipes to the phase process: one it reads paths from, one line each, and
#    one it writes events to. A path is handed to a worker only when it has
#    finished the one before, so the files are shared out one at a time and
#    a slow file holds up only its own worker. A worker requires each file
#    it is given, as Rake's loader requires them. After its last file a
#    Minitest worker ends as a program ends, so the `at_exit` a framework
#    registered (`minitest/autorun`) runs its tests; an RSpec worker runs the
#    examples its files defined through RSpec's own runner, as `rspec` does
#    (`RSpec::Core::Runner.invoke`), and exits with its status.
# 4. Results: a worker writes one JSON line per event, through
#    `shims/test_worker.rb`, loaded before this file: `done` once a file is
#    loaded, with the exception loading raised or `nil`, and, from the
#    Minitest reporter and the RSpec formatter that shim registers, one
#    `pass|fail|error|skip` line per test or example. Here they are counted
#    for #94's aggregator, which replaces the reporting below; the verdict
#    stays the workers' own, below, and a line that is not an event is
#    skipped.
# 5. A worker that ends before it says `done` for the file it was given —
#    killed by a signal, or exiting, with any status — is reported with that
#    file, and a new worker is forked for the files still waiting, so one
#    crash costs one file, not the rest of the suite. A worker that ends by a
#    signal or a failing status after its last file is reported too, with
#    the files it ran, and so is a file whose loading raised. Any of these,
#    or spec files when `rspec/core` cannot be loaded, fails its phase, and
#    the phase process says so through its exit status; either phase
#    failing makes the exit status 1.
#
# A phase process and the runner end with `exit!`, so an `at_exit` a helper
# registered runs in the workers, which ran the tests, and not again in a
# process with none loaded.

require "json"

module Spinel
  module TestRunner
    Worker = Struct.new(:number, :pid, :tasks, :events, :buffer, :file, :ran)

    # The events a test or an example ends with.
    RESULTS = %w[pass fail error skip].freeze

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
      failed = false
      failed = true if !files.empty? && !phase { run_tests(files, jobs) }
      failed = true if !specs.empty? && !phase { run_specs(specs, jobs) }
      failed ? 1 : 0
    end

    # The block, which answers true when its files failed, in a process of
    # its own; true when that process passed.
    def self.phase
      pid = fork do
        failed = yield
        $stdout.flush
        $stderr.flush
        exit!(failed ? 1 : 0)
      end
      _, status = Process.wait2(pid)
      status.success?
    end

    # `dirs` that exist, each put first on `$LOAD_PATH` in turn.
    def self.load_paths(dirs)
      dirs.each do |dir|
        path = File.expand_path(dir)
        $LOAD_PATH.unshift(path) if File.directory?(path) && !$LOAD_PATH.include?(path)
      end
    end

    # The Minitest files, on up to `jobs` workers; true when they failed.
    def self.run_tests(files, jobs)
      load_paths(%w[test lib])
      require File.expand_path("test/test_helper.rb") if File.file?("test/test_helper.rb")
      run_files(files, jobs, :minitest)
    end

    # The RSpec files, on up to `jobs` workers; true when they failed.
    # `spec/` ends up before `lib/`, as `rspec`'s `requires=` leaves them.
    def self.run_specs(files, jobs)
      load_paths(%w[lib spec])
      begin
        require "rspec/core"
      rescue LoadError => e
        warn "spinel test: #{files.size} RSpec file#{"s" unless files.size == 1} under spec/ not run: #{e.message}"
        return true
      end
      require File.expand_path("spec/spec_helper.rb") if File.file?("spec/spec_helper.rb")
      run_files(files, jobs, :rspec)
    end

    # Run `files` on up to `jobs` workers of `framework`; true when they
    # failed.
    def self.run_files(files, jobs, framework)
      queue = files.dup
      workers = {}
      failed = false
      # How many tests or examples ended each way, from every worker.
      counts = Hash.new(0)
      count = 0
      add_worker = lambda do
        count += 1
        worker = start(count, workers, framework)
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
              failed = true unless line_from(worker, line, queue, counts)
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
    def self.start(number, workers, framework)
      tasks_r, tasks_w = IO.pipe
      events_r, events_w = IO.pipe
      pid = fork do
        workers.each_value do |other|
          other.tasks.close unless other.tasks.closed?
          other.events.close
        end
        tasks_w.close
        events_r.close
        work(tasks_r, events_w, framework)
      end
      tasks_r.close
      events_w.close
      tasks_w.sync = true
      Worker.new(number, pid, tasks_w, events_r, +"", nil, [])
    end

    # The worker: each path read is a file to require, then a `done` event.
    # A Minitest worker then has its reporter added to the run the `at_exit`
    # its helper registered makes; an RSpec worker runs what its files
    # defined, its formatter listening.
    def self.work(tasks, events, framework)
      TestWorker.start(events)
      while (line = tasks.gets)
        file = line.chomp
        error = nil
        begin
          require File.expand_path(file)
        rescue StandardError, ScriptError => e
          error = "#{e.class}: #{e.message}"
        end
        TestWorker.emit({ "event" => "done", "file" => file, "error" => error })
      end
      case framework
      when :minitest then TestWorker.minitest
      when :rspec then run_examples
      end
    end

    # `rspec`'s own end, `RSpec::Core::Runner.invoke`, over the example
    # groups this worker's files defined. With no paths given, and the
    # command not `rspec`, RSpec loads no file of its own (`load_spec_files`
    # counts the ones already loaded), and it reads `.rspec` and `SPEC_OPTS`
    # as it always does.
    def self.run_examples
      TestWorker.rspec
      RSpec::Core::Runner.disable_autorun!
      status = RSpec::Core::Runner.run([], $stderr, $stdout).to_i
      exit(status) if status != 0
    end

    # One line from `worker`; false when it says the suite failed. A line
    # that is not an event is skipped. Each is also appended, as it came, to
    # the file `SPINEL_TEST_EVENTS` names, when it names one.
    # ponytail: the raw stream for tools until `--reporter json` (#95).
    def self.line_from(worker, line, queue, counts)
      if (path = ENV["SPINEL_TEST_EVENTS"]) && !path.empty?
        File.open(path, "a") { |file| file.write(line) }
      end
      parsed = begin
        JSON.parse(line)
      rescue JSON::ParserError
        nil
      end
      return true unless parsed.is_a?(Hash)

      event(worker, parsed, queue, counts)
    end

    # One event from `worker`, a result counted in `counts`; false when a
    # file it was given failed to load.
    def self.event(worker, event, queue, counts)
      kind = event["event"]
      counts[kind] += 1 if RESULTS.include?(kind)
      return true unless kind == "done"

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
