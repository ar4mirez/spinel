# Reading a file a line at a time with `IO#gets`, at four sizes, so a loop
# that is quadratic in the file shows as one (#585). Each file holds that many
# lines of "hello world line\n"; 16000 of them are past one fill of the read
# buffer.
#
#   scripts/bench.sh io_gets
#   target/release/spinel run bench/io_gets.rb
#   ruby --yjit bench/io_gets.rb
#
# One file for both sides, so neither can quietly measure something else.
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

require "tmpdir"

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

Dir.mktmpdir("spinel-bench-io-gets") do |dir|
  [1000, 2000, 4000, 16000].each do |k|
    n = File.join(dir, "lines-#{k}")
    File.write(n, "hello world line\n" * k)
    time("gets          k=#{k}") { File.open(n) { |f| nil while f.gets } }
    time("gets UTF-16LE k=#{k}") { File.open(n, "r:UTF-8:UTF-16LE") { |f| nil while f.gets } }
  end
end
