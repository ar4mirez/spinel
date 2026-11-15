# Small writes to a file, which the write buffer (#41) holds until it has
# 8 KiB to send: `write` of one and of three Strings, `puts`, `print` and
# `<<`, against `syswrite`, which is one `write(2)` every time, and the same
# `write` with `sync = true`, which writes through. Each row writes `n` times
# to a file of its own and closes it, so the last flush is timed too.
#
#   scripts/bench.sh io_write
#   target/release/spinel run bench/io_write.rb
#   ruby --yjit bench/io_write.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

require "tmpdir"

N = 20_000
LINE = "hello world line"

def time(label, dir)
  path = File.join(dir, label.tr(" ", "_"))
  File.open(path, "w") do |f|
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield f
    f.close
    elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
    puts "#{label.ljust(18)} n=#{N}: #{(elapsed * 1000).round(1)} ms, #{(elapsed * 1e6 / N).round(2)} us/op"
  end
end

Dir.mktmpdir("spinel-bench-io-write") do |dir|
  time("write 1 arg", dir) { |f| N.times { f.write(LINE) } }
  time("write 3 args", dir) { |f| N.times { f.write(LINE, " ", LINE) } }
  time("puts", dir) { |f| N.times { f.puts(LINE) } }
  time("print", dir) { |f| N.times { f.print(LINE) } }
  time("<<", dir) { |f| N.times { f << LINE } }
  time("syswrite", dir) { |f| N.times { f.syswrite(LINE) } }
  time("write sync", dir) { |f| f.sync = true; N.times { f.write(LINE) } }
end
