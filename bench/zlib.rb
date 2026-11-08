# The built-in zlib (#59): a megabyte of text-like bytes deflated and
# inflated whole, at the default level and at the fastest, gzipped and
# gunzipped, and the two per-call paths a program streams through, a
# GzipWriter fed a line at a time and a GzipReader read a line at a time.
#
#   target/release/spinel run bench/zlib.rb
#   ruby --yjit bench/zlib.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).
require "zlib"
require "stringio"

# The best of five: the quantity is the time with nothing else interfering.
def time(label)
  best = (1..5).map do
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end.min
  puts "#{label}: #{(best * 1000).round(2)} ms"
end

words = %w[alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu]
x = 1
text = +""
while text.bytesize < 1_000_000
  x = (x * 214013 + 2531011) & 0x7fffffff
  text << words[(x >> 16) % words.size] << ((x & 7) == 0 ? "\n" : " ")
end
deflated = Zlib.deflate(text)
gzipped = Zlib.gzip(text)
lines = text.lines.first(10_000)
puts "input: #{text.bytesize} bytes, deflated #{deflated.bytesize}, #{lines.size} lines"

time("Zlib.deflate        ") { Zlib.deflate(text) }
time("Zlib.deflate level 1") { Zlib.deflate(text, 1) }
time("Zlib.inflate        ") { Zlib.inflate(deflated) }
time("Zlib.gzip           ") { Zlib.gzip(text) }
time("Zlib.gunzip         ") { Zlib.gunzip(gzipped) }
time("Zlib.crc32          ") { Zlib.crc32(text) }
time("GzipWriter#puts     ") do
  gz = Zlib::GzipWriter.new(StringIO.new(+""))
  lines.each { |line| gz.puts(line) }
  gz.close
end
time("GzipReader#each_line") do
  Zlib::GzipReader.new(StringIO.new(gzipped)).each_line.first(10_000)
end
