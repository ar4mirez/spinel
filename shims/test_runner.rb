# Spinel's test runner (docs/test-runner.md, "Execution" and "Output"): what
# `spinel test` runs on the VM, after `shims/setup.rb` and the gem prelude,
# as `spinel run` runs a program. `ARGV` is the worker count the CLI chose
# (`-j`, else the physical cores), then the files it was given, each with
# an optional `:LINE` after it; it is cleared before any test code can read
# it.
#
# 1. Discovery: with no paths, `test/**/*_test.rb` (Minitest) and
#    `spec/**/*_spec.rb` (RSpec), relative to the current directory, each
#    sorted; a file given is a spec file when its name ends `_spec.rb`, else
#    a Minitest file, and a path given that is not a file is an error. The
#    Minitest files run first, then the RSpec files. Each phase runs in a
#    process of its own forked from this one, the RSpec one only after the
#    Minitest one has ended, so neither framework's helper or `at_exit`
#    reaches the other's workers.
# 2. Minitest: the phase process puts `lib/` and `test/` on `$LOAD_PATH`, as
#    Rake's TestTask puts them, and requires `test/test_helper.rb` once,
#    before any worker exists: whatever it boots, every worker inherits
#    through `fork` instead of booting again. RSpec (#837): the phase
#    process puts `lib/` and `spec/` on `$LOAD_PATH`, as the `rspec` command
#    puts them, and requires `rspec/core` and `spec/spec_helper.rb` once,
#    the same way.
# 3. Workers: each phase process forks its workers after the helpers, up to
#    the worker count, never more than there are files. Each worker has two
#    pipes to the phase process: one it reads files from, one line each,
#    `path` or `path:LINE`, and one it writes events to. A file is handed to
#    a worker only when it has finished the one before, so the files are
#    shared out one at a time and a slow file holds up only its own worker.
#    A worker requires each file it is given, as Rake's loader requires
#    them. After its last file a Minitest worker ends as a program ends, so
#    the `at_exit` a framework registered (`minitest/autorun`) runs its
#    tests; an RSpec worker runs the examples its files defined through
#    RSpec's own runner, as `rspec` does (`RSpec::Core::Runner.invoke`), and
#    exits with its status. A file given with a line runs only the tests or
#    examples at it (`shims/test_worker.rb`).
# 4. Results: a worker writes one JSON line per event, through
#    `shims/test_worker.rb`, loaded before this file: `done` once a file is
#    loaded, with the exception loading raised or `nil`, and, from the
#    Minitest reporter and the RSpec formatter that shim registers, one
#    `pass|fail|error|skip` line per test or example, in place of each
#    framework's own report. The phase process prints a character for each
#    result as it comes (`.` a pass, `F` a failure, `E` an error, `S` a
#    Minitest skip and `*` a pending RSpec example, as each framework's
#    progress prints them), keeps each failure and error, and counts every
#    result; a line that is not an event is skipped.
# 5. A worker that ends before it says `done` for the file it was given —
#    killed by a signal, or exiting, with any status — is a problem reported
#    with that file, and a new worker is forked for the files still waiting,
#    so one crash costs one file, not the rest of the suite. So is a worker
#    that ends by a signal, or by a failing status with no failure or error
#    reported to account for it, after its last file, with the files it ran;
#    a file whose loading raised; and spec files when `rspec/core` cannot be
#    loaded.
# 6. Each phase process sends what it kept to this one, as one JSON
#    document on a pipe of its own, and this one prints the report once
#    both have ended: each failure and error with the `file:line` of its
#    test and its backtrace, trimmed, with each run of gem frames folded to
#    one line; the problems, on stderr; a summary line counting each kind;
#    and a `spinel test` command that runs each failure again, pasted as it
#    is, from the location its event names to run it again (`rerun`). The exit status is 1 when anything failed, raised or
#    went wrong, and 0 otherwise.
#
# A phase process and the runner end with `exit!`, so an `at_exit` a helper
# registered runs in the workers, which ran the tests, and not again in a
# process with none loaded.

require "json"
require "shellwords"
require "stringio"

module Spinel
  module TestRunner
    Worker = Struct.new(:number, :pid, :tasks, :events, :buffer, :file, :ran, :failed)

    # The events a test or an example ends with, each as the progress line
    # prints it; a skip is `S` for Minitest and `*` for RSpec, as each of
    # theirs prints it.
    RESULTS = %w[pass fail error skip].freeze
    PROGRESS = {
      minitest: { "pass" => ".", "fail" => "F", "error" => "E", "skip" => "S" },
      rspec: { "pass" => ".", "fail" => "F", "error" => "E", "skip" => "*" },
    }.freeze

    # A backtrace frame inside an installed gem: `gems/NAME-VERSION/` under a
    # GEM_HOME or a Bundler path, or a Bundler git checkout.
    GEM_FRAME = %r{(?:\A|/)(?:gems/[^/]+-\d[^/]*|bundler/gems/[^/]+)/}

    # How many frames of a backtrace that are not in gems are printed.
    FRAMES = 10

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

    # `path` or `path:LINE`: the path and its line, or nil.
    def self.location(arg)
      if (match = arg.match(/\A(.+):(\d+)\z/))
        [match[1], match[2].to_i]
      else
        [arg, nil]
      end
    end

    # A file as the runner names it to workers: its path, then its line.
    def self.task(path, line)
      line ? "#{path}:#{line}" : path
    end

    # A path under the current directory from it, as discovery names files.
    def self.relative(path)
      File.expand_path(path).delete_prefix("#{Dir.pwd}/")
    end

    # The Minitest and RSpec files to run, each `[path, line]`, line nil for
    # every test, from the files given, else discovered; nil, after saying
    # why, when there are none or a path given is not one file once.
    def self.discover(args)
      found = {}
      if args.empty?
        Dir.glob("test/**/*_test.rb").sort.each { |path| found[path] = nil }
        Dir.glob("spec/**/*_spec.rb").sort.each { |path| found[path] = nil }
        if found.empty?
          warn "spinel test: no test files: nothing matches test/**/*_test.rb or spec/**/*_spec.rb"
          return nil
        end
      else
        args.each do |arg|
          path, line = location(arg)
          unless File.file?(path)
            warn "spinel test: no such file: #{path}"
            return nil
          end
          path = relative(path)
          if found.key?(path)
            warn "spinel test: #{path} is given more than once"
            return nil
          end
          found[path] = line
        end
      end
      found.partition { |path, _| !path.end_with?("_spec.rb") }
    end

    def self.run(jobs, args)
      tests, specs = discover(args)
      return 1 unless tests

      report = { "counts" => Hash.new(0), "failures" => [], "problems" => [] }
      printed = false
      unless tests.empty?
        printed = true if phase(report, "Minitest") { |kept| run_tests(tests, jobs, kept) }
      end
      unless specs.empty?
        printed = true if phase(report, "RSpec") { |kept| run_specs(specs, jobs, kept) }
      end
      print_report(report, printed)
    end

    # The block, given a report to keep what its files did in, in a process
    # of its own; what it kept is added to `report`. True when it printed
    # any progress.
    def self.phase(report, name)
      reader, writer = IO.pipe
      pid = fork do
        reader.close
        kept = { "counts" => Hash.new(0), "failures" => [], "problems" => [] }
        yield kept
        $stdout.flush
        $stderr.flush
        writer.write(JSON.generate(kept))
        writer.close
        exit!(0)
      end
      writer.close
      sent = reader.read
      reader.close
      _, status = Process.wait2(pid)
      kept = begin
        JSON.parse(sent)
      rescue JSON::ParserError
        nil
      end
      unless status.success? && kept.is_a?(Hash)
        report["problems"] << { "message" => "the #{name} run (pid #{pid}) #{ending(status)} before it reported" }
        return false
      end
      kept["counts"].each { |kind, count| report["counts"][kind] += count }
      # Each phase's by where its tests are, as they came from several
      # workers in no order of their own.
      report["failures"].concat(
        kept["failures"].sort_by.with_index { |event, index| [event["file"].to_s, event["line"].to_i, index] }
      )
      report["problems"].concat(kept["problems"])
      kept["counts"].values.sum.positive?
    end

    # `dirs` that exist, each put first on `$LOAD_PATH` in turn.
    def self.load_paths(dirs)
      dirs.each do |dir|
        path = File.expand_path(dir)
        $LOAD_PATH.unshift(path) if File.directory?(path) && !$LOAD_PATH.include?(path)
      end
    end

    # The Minitest files, on up to `jobs` workers.
    def self.run_tests(files, jobs, report)
      load_paths(%w[test lib])
      require File.expand_path("test/test_helper.rb") if File.file?("test/test_helper.rb")
      run_files(files, jobs, :minitest, report)
    end

    # The RSpec files, on up to `jobs` workers. `spec/` ends up before
    # `lib/`, as `rspec`'s `requires=` leaves them.
    def self.run_specs(files, jobs, report)
      load_paths(%w[lib spec])
      begin
        require "rspec/core"
      rescue LoadError => e
        report["problems"] << {
          "message" => "#{files.size} RSpec file#{"s" unless files.size == 1} under spec/ not run: #{e.message}",
        }
        return
      end
      require File.expand_path("spec/spec_helper.rb") if File.file?("spec/spec_helper.rb")
      run_files(files, jobs, :rspec, report)
    end

    # Run `files` on up to `jobs` workers of `framework`, keeping what they
    # did in `report`.
    def self.run_files(files, jobs, framework, report)
      queue = files.map { |path, line| task(path, line) }
      workers = {}
      count = 0
      add_worker = lambda do
        count += 1
        worker = start(count, workers, framework)
        workers[worker.events] = worker
        hand_out(worker, queue)
      end
      [jobs, queue.size].min.times { add_worker.call }

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
              line_from(worker, worker.buffer.slice!(0..newline), queue, framework, report)
            end
            next
          end

          # Its end of the events pipe is closed: the worker has ended.
          workers.delete(io)
          io.close
          worker.tasks.close unless worker.tasks.closed?
          _, status = Process.wait2(worker.pid)
          if worker.file
            report["problems"] << {
              "message" => "worker #{worker.number} (pid #{worker.pid}) #{ending(status)} while running #{worker.file}",
              "rerun" => worker.file,
            }
            add_worker.call unless queue.empty?
          elsif status.signaled? || (!status.success? && worker.failed.zero?)
            # A crash while the `at_exit` ran the tests, or a failing status
            # no failed test accounts for (`minitest/autorun` exits 1 when
            # one failed).
            ran = worker.ran.empty? ? "no files" : worker.ran.join(", ")
            report["problems"] << {
              "message" => "worker #{worker.number} (pid #{worker.pid}) #{ending(status)} after its last file; it ran #{ran}",
            }
          end
        end
      end
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
      Worker.new(number, pid, tasks_w, events_r, +"", nil, [], 0)
    end

    # The worker: each line read is a file to require, then a `done` event.
    # A Minitest worker then has its reporter added to the run the `at_exit`
    # its helper registered makes; an RSpec worker runs what its files
    # defined, its formatter listening. Each is told the line asked for.
    def self.work(tasks, events, framework)
      TestWorker.start(events)
      loaded = []
      while (task = tasks.gets)
        path, line = location(task.chomp)
        before = TestWorker.runnables
        error = nil
        begin
          require File.expand_path(path)
        rescue StandardError, ScriptError => e
          error = "#{e.class}: #{e.message}"
        end
        loaded << [path, line, TestWorker.runnables - before]
        TestWorker.emit({ "event" => "done", "file" => path, "error" => error })
      end
      case framework
      when :minitest then TestWorker.minitest(loaded)
      when :rspec then run_examples(loaded.filter_map { |path, line, _| [path, line] if line })
      end
    end

    # `rspec`'s own end, `RSpec::Core::Runner.invoke`, over the example
    # groups this worker's files defined. With no paths given, and the
    # command not `rspec`, RSpec loads no file of its own (`load_spec_files`
    # counts the ones already loaded), and it reads `.rspec` and `SPEC_OPTS`
    # as it always does. What its formatters print goes to a buffer, as the
    # runner reports the run; it is shown only when the run fails with no
    # failed example to say why, such as an error outside any example.
    def self.run_examples(locations)
      TestWorker.rspec(locations)
      RSpec::Core::Runner.disable_autorun!
      out = StringIO.new
      status = RSpec::Core::Runner.run([], $stderr, out).to_i
      return if status.zero?

      $stderr.write(out.string) if TestWorker.failed.zero?
      exit(status)
    end

    # One line from `worker`. A line that is not an event is skipped. Each
    # is also appended, as it came, to the file `SPINEL_TEST_EVENTS` names,
    # when it names one.
    # ponytail: the raw stream for tools until `--reporter json` (#95).
    def self.line_from(worker, line, queue, framework, report)
      if (path = ENV["SPINEL_TEST_EVENTS"]) && !path.empty?
        File.open(path, "a") { |file| file.write(line) }
      end
      parsed = begin
        JSON.parse(line)
      rescue JSON::ParserError
        nil
      end
      event(worker, parsed, queue, framework, report) if parsed.is_a?(Hash)
    end

    # One event from `worker`: a result is printed as progress and counted,
    # and kept when it failed; a file that failed to load is a problem.
    def self.event(worker, event, queue, framework, report)
      kind = event["event"]
      if RESULTS.include?(kind)
        report["counts"][kind] += 1
        $stdout.write(PROGRESS[framework][kind])
        $stdout.flush
        if kind == "fail" || kind == "error"
          worker.failed += 1
          report["failures"] << event
        end
        return
      end
      return unless kind == "done"

      worker.ran << worker.file
      worker.file = nil
      hand_out(worker, queue)
      return unless event["error"]

      report["problems"] << { "message" => "#{event["file"]}: #{event["error"]}", "rerun" => event["file"] }
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

    # "1 failure", "2 failures".
    def self.count(number, noun)
      "#{number} #{noun}#{"s" unless number == 1}"
    end

    # `backtrace` as printed: each run of gem frames folded to one line,
    # and no more than `FRAMES` of the others, the rest counted.
    def self.trimmed(backtrace)
      lines = []
      shown = 0
      folded = 0
      backtrace.each_with_index do |frame, index|
        if frame.match?(GEM_FRAME)
          folded += 1
          next
        end
        if folded.positive?
          lines << "(#{count(folded, "gem frame")})"
          folded = 0
        end
        if shown == FRAMES
          lines << "(#{count(backtrace.size - index, "more frame")})"
          return lines
        end
        lines << frame
        shown += 1
      end
      lines << "(#{count(folded, "gem frame")})" if folded.positive?
      lines
    end

    # The command that runs `location`, `path` or `path:LINE`, again.
    def self.rerun(location, comment = nil)
      command = "spinel test #{Shellwords.escape(location)}"
      comment ? "#{command} # #{comment.gsub(/\s+/, " ")}" : command
    end

    # The report, once every phase has ended; the exit status.
    def self.print_report(report, printed)
      counts = report["counts"]
      failures = report["failures"]
      problems = report["problems"]
      out = +""
      out << "\n" if printed

      failures.each_with_index do |event, index|
        kind = event["event"] == "error" ? "Error" : "Failure"
        where = event["file"] ? " [#{[event["file"], event["line"]].compact.join(":")}]" : ""
        out << "\n  #{index + 1}) #{kind}: #{event["name"]}#{where}\n"
        message = event["message"].to_s.sub(/\A\n+/, "").rstrip
        message.each_line { |text| out << (text.strip.empty? ? "\n" : "     #{text.chomp}\n") }
        trimmed(event["backtrace"] || []).each { |frame| out << "     # #{frame}\n" }
      end
      $stdout.write(out)
      $stdout.flush

      problems.each { |problem| warn "spinel test: #{problem["message"]}" }
      $stderr.flush

      summary = [
        count(RESULTS.sum { |kind| counts[kind] }, "test"),
        count(counts["fail"], "failure"),
        count(counts["error"], "error"),
        count(counts["skip"], "skip"),
      ]
      summary << count(problems.size, "problem") unless problems.empty?
      out = +"\n#{summary.join(", ")}\n"

      reruns = failures.filter_map do |event|
        rerun(event["rerun"], event["name"]) if event["rerun"]
      end
      reruns.concat(problems.filter_map { |problem| rerun(problem["rerun"]) if problem["rerun"] })
      reruns.uniq! { |command| command.sub(/ # .*\z/, "") }
      unless reruns.empty?
        out << "\nRe-run each failure:\n\n"
        reruns.each { |command| out << "#{command}\n" }
      end
      $stdout.write(out)
      $stdout.flush
      failures.empty? && problems.empty? ? 0 : 1
    end
  end
end

jobs = Integer(ARGV.shift)
args = ARGV.dup
ARGV.clear
code = Spinel::TestRunner.run(jobs, args)
$stdout.flush
$stderr.flush
exit!(code)
