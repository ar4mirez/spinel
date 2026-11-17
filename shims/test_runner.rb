# Spinel's test runner (docs/test-runner.md, "Execution" and "Output"): what
# `spinel test` runs on the VM, after `shims/setup.rb` and the gem prelude,
# as `spinel run` runs a program. `ARGV` is the worker count the CLI chose
# (`-j`, else the physical cores), the reporter (`--reporter`: `progress`,
# `json` or `junit`), then the files it was given, each with an optional
# `:LINE` after it; it is cleared before any test code can read it.
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
#    is, from the location its event names to run it again (`rerun`). The
#    exit status is 1 when anything failed, raised or went wrong, and 0
#    otherwise, whatever the reporter.
# 7. `--reporter json` (#95) prints, in place of the progress and the
#    report, each event as it is read, one JSON object per line, then a
#    `problem` event for each problem and a `summary` event. `--reporter
#    junit` prints one JUnit XML document once both phases have ended, a
#    `testsuite` per file. Either way stdout gets that and nothing else:
#    the runner's stdout is moved to stderr before any helper or test can
#    write to it, so what they print goes there, and the problems are still
#    told on stderr too.
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

    # What `--reporter` chose, and where what it prints goes: stdout, or,
    # for `json` and `junit`, a copy of it once stdout itself is stderr.
    @reporter = "progress"
    @out = $stdout

    # Each character XML 1.0 cannot hold, and each that needs escaping in
    # an attribute's value or in text. A newline, a return or a tab in an
    # attribute is written as a reference, as a parser reads a plain one
    # there as a space.
    XML_UNHELD = /[\x00-\x08\x0B\x0C\x0E-\x1F\uFFFE\uFFFF]/
    XML_ESCAPES = {
      "&" => "&amp;", "<" => "&lt;", ">" => "&gt;", '"' => "&quot;",
      "\n" => "&#10;", "\r" => "&#13;", "\t" => "&#9;",
    }.freeze

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

    def self.run(jobs, reporter, args)
      @reporter = reporter
      unless reporter == "progress"
        @out = $stdout.dup
        $stdout.reopen($stderr)
      end
      tests, specs = discover(args)
      return 1 unless tests

      report = { "counts" => Hash.new(0), "failures" => [], "problems" => [], "results" => [] }
      printed = false
      unless tests.empty?
        printed = true if phase(report, "Minitest") { |kept| run_tests(tests, jobs, kept) }
      end
      unless specs.empty?
        printed = true if phase(report, "RSpec") { |kept| run_specs(specs, jobs, kept) }
      end
      case reporter
      when "json" then json_report(report)
      when "junit" then junit_report(report)
      else print_report(report, printed)
      end
    end

    # `events` in order of where their tests are, as they came from several
    # workers in no order of their own.
    def self.by_location(events)
      events.sort_by.with_index { |event, index| [event["file"].to_s, event["line"].to_i, index] }
    end

    # The block, given a report to keep what its files did in, in a process
    # of its own; what it kept is added to `report`. True when it printed
    # any progress.
    def self.phase(report, name)
      reader, writer = IO.pipe
      pid = fork do
        reader.close
        kept = { "counts" => Hash.new(0), "failures" => [], "problems" => [] }
        # Every result, for a report that lists them all.
        kept["results"] = [] if @reporter == "junit"
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
      # Each phase's by where its tests are.
      report["failures"].concat(by_location(kept["failures"]))
      report["results"].concat(by_location(kept["results"]).map { |event| [name, event] }) if kept["results"]
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

    # One line from `worker`. A line that is not an event is skipped.
    def self.line_from(worker, line, queue, framework, report)
      parsed = begin
        JSON.parse(line)
      rescue JSON::ParserError
        nil
      end
      event(worker, parsed, queue, framework, report) if parsed.is_a?(Hash)
    end

    # One event from `worker`: a result is printed as progress, or as
    # itself for `--reporter json`, and counted, and kept when it failed, or
    # whatever it was for `--reporter junit`; a file that failed to load is
    # a problem.
    def self.event(worker, event, queue, framework, report)
      kind = event["event"]
      if RESULTS.include?(kind)
        report["counts"][kind] += 1
        case @reporter
        when "progress"
          @out.write(PROGRESS[framework][kind])
          @out.flush
        when "json" then emit(event)
        when "junit" then report["results"] << event
        end
        if kind == "fail" || kind == "error"
          worker.failed += 1
          report["failures"] << event
        end
        return
      end
      return unless kind == "done"

      emit(event) if @reporter == "json"

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

    # `event` as one line of JSON, for `--reporter json`.
    def self.emit(event)
      @out.write("#{JSON.generate(event)}\n")
      @out.flush
    end

    # Each problem, on stderr.
    def self.warn_problems(problems)
      problems.each { |problem| warn "spinel test: #{problem["message"]}" }
      $stderr.flush
    end

    # The exit status: 1 when anything failed, raised or went wrong.
    def self.status(report)
      report["failures"].empty? && report["problems"].empty? ? 0 : 1
    end

    # The counts the summary gives, by the word it gives each with.
    def self.summary(report)
      counts = report["counts"]
      {
        "tests" => RESULTS.sum { |kind| counts[kind] },
        "failures" => counts["fail"],
        "errors" => counts["error"],
        "skips" => counts["skip"],
        "problems" => report["problems"].size,
      }
    end

    # `--reporter json`'s end, once every phase has ended, after the events
    # it printed as they came: a `problem` event for each problem, with the
    # file to run again or `null`, then a `summary` event with the counts;
    # the exit status.
    def self.json_report(report)
      warn_problems(report["problems"])
      report["problems"].each do |problem|
        emit({ "event" => "problem", "message" => problem["message"], "rerun" => problem["rerun"] })
      end
      emit({ "event" => "summary" }.merge(summary(report)))
      status(report)
    end

    # `text` for XML: each character XML cannot hold as `\uXXXX`, and the
    # rest escaped for an attribute's value or text alike.
    def self.xml(text)
      text.to_s.scrub.gsub(XML_UNHELD) { |char| format("\\u%04X", char.ord) }.gsub(/[&<>"\n\r\t]/, XML_ESCAPES)
    end

    # ` name="value"` for each attribute that has a value.
    def self.xml_attributes(attributes)
      attributes.filter_map { |name, value| " #{name}=\"#{xml(value)}\"" unless value.nil? }.join
    end

    # Seconds, from milliseconds, as JUnit's `time` gives them.
    def self.seconds(ms)
      format("%.6f", ms.to_f / 1000)
    end

    # One `testcase`: `[attributes, element]`, `element` a `failure`,
    # `error` or `skipped` (`[name, message, text]`), or nil for a pass.
    def self.xml_testcase(out, attributes, element)
      out << "    <testcase#{xml_attributes(attributes)}"
      if element
        tag, message, text = element
        out << ">\n      <#{tag}#{xml_attributes("message" => message)}"
        # Text, its line ends written as they are: in text a parser keeps them.
        out << (text.empty? ? "/>\n" : ">#{xml(text).gsub("&#10;", "\n")}</#{tag}>\n")
        out << "    </testcase>\n"
      else
        out << "/>\n"
      end
    end

    # A result event's `testcase`. Minitest's `classname` is the test's
    # class and its name the method, as Minitest's own JUnit reporters give
    # them; RSpec's `classname` is its file's path with dots for slashes and
    # no `.rb`, and its name the example's full description, as
    # rspec_junit_formatter gives them.
    def self.result_testcase(out, framework, event)
      if framework == "Minitest"
        classname, name = event["name"].to_s.split("#", 2)
        name ||= classname
      else
        classname = event["file"].to_s.delete_suffix(".rb").tr("/", ".")
        name = event["name"]
      end
      attributes = {
        "name" => name, "classname" => classname, "file" => event["file"],
        "line" => event["line"], "time" => seconds(event["ms"]),
      }
      message = event["message"].to_s.sub(/\A\n+/, "").rstrip
      element =
        case event["event"]
        when "fail" then ["failure", message, ([message] + (event["backtrace"] || [])).join("\n")]
        when "error" then ["error", message, ([message] + (event["backtrace"] || [])).join("\n")]
        when "skip" then ["skipped", message, ""]
        end
      xml_testcase(out, attributes, element)
    end

    # `--reporter junit`'s report, once every phase has ended: one JUnit XML
    # document, the shape CI systems read (Jenkins' JUnit schema), with a
    # `testsuite` per file, the Minitest ones first, each by where its tests
    # are, and a `testcase` per test or example, with a `failure`, `error`
    # or `skipped` when it did not pass. Each problem is a `testcase` with
    # an `error` too, in a last suite, `spinel test`, so CI shows it; the
    # problems are told on stderr as well. The exit status.
    def self.junit_report(report)
      warn_problems(report["problems"])
      suites = []
      report["results"].each do |framework, event|
        suite = suites.last
        unless suite && suite[:name] == event["file"].to_s
          suite = { name: event["file"].to_s, cases: [], tests: 0, failures: 0, errors: 0, skipped: 0, ms: 0.0 }
          suites << suite
        end
        suite[:cases] << [framework, event]
        suite[:tests] += 1
        suite[:ms] += event["ms"].to_f
        case event["event"]
        when "fail" then suite[:failures] += 1
        when "error" then suite[:errors] += 1
        when "skip" then suite[:skipped] += 1
        end
      end
      problems = report["problems"]
      timestamp = Time.now.utc.strftime("%Y-%m-%dT%H:%M:%S")
      total = suites.sum { |suite| suite[:tests] } + problems.size
      failures = suites.sum { |suite| suite[:failures] }
      errors = suites.sum { |suite| suite[:errors] } + problems.size
      ms = suites.sum { |suite| suite[:ms] }

      out = +"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
      out << "<testsuites#{xml_attributes(
        "name" => "spinel test", "tests" => total, "failures" => failures, "errors" => errors, "time" => seconds(ms)
      )}>\n"
      suites.each do |suite|
        out << "  <testsuite#{xml_attributes(
          "name" => suite[:name], "tests" => suite[:tests], "failures" => suite[:failures],
          "errors" => suite[:errors], "skipped" => suite[:skipped], "time" => seconds(suite[:ms]),
          "timestamp" => timestamp
        )}>\n"
        suite[:cases].each { |framework, event| result_testcase(out, framework, event) }
        out << "  </testsuite>\n"
      end
      unless problems.empty?
        out << "  <testsuite#{xml_attributes(
          "name" => "spinel test", "tests" => problems.size, "failures" => 0, "errors" => problems.size,
          "skipped" => 0, "time" => seconds(0), "timestamp" => timestamp
        )}>\n"
        problems.each do |problem|
          attributes = {
            "name" => problem["rerun"] || problem["message"], "classname" => "spinel test",
            "file" => problem["rerun"], "time" => seconds(0),
          }
          xml_testcase(out, attributes, ["error", problem["message"], ""])
        end
        out << "  </testsuite>\n"
      end
      out << "</testsuites>\n"
      @out.write(out)
      @out.flush
      status(report)
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
      @out.write(out)
      @out.flush

      warn_problems(problems)

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
      @out.write(out)
      @out.flush
      status(report)
    end
  end
end

jobs = Integer(ARGV.shift)
reporter = ARGV.shift
args = ARGV.dup
ARGV.clear
code = Spinel::TestRunner.run(jobs, reporter, args)
$stdout.flush
$stderr.flush
exit!(code)
