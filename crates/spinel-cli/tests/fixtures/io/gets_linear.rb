# How the time a `gets` loop takes grows with what it reads, for each read
# #585 found quadratic: a line at a time, through a transcoder, a record at a
# time in paragraph mode, and one long paragraph into a BOM'd UTF-16. Each is
# timed on a short file and on one eight times as long, both within one fill
# of the read buffer, and the ratio is printed: about 8 where the loop is
# linear, several times that where each search of the buffer costs all of it.
dir = ARGV[0]

def seconds
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end

# The best of a few, so a pause elsewhere does not count as the loop's.
def ratio(dir, name, unit, short, mode, separator, tries = 2)
  times = [short, short * 8].map do |count|
    path = File.join(dir, "#{name}-#{count}")
    File.binwrite(path, unit * count)
    (1..tries).map do
      seconds { File.open(path, mode) { |f| nil while (separator.nil? ? f.gets : f.gets(separator)) } }
    end.min
  end
  puts "#{name} #{(times[1] / times[0]).round(1)}"
end

ratio(dir, "lines", "hello world line\n", 400, "r", nil)
ratio(dir, "transcoded", "hello world line\n", 200, "r:UTF-8:UTF-16LE", nil)
ratio(dir, "paragraphs", "hello world\nline\n\n", 200, "r", "")
# A line costs the most here, so it is read once.
ratio(dir, "long-paragraph", "hello world line\n", 100, "r:UTF-8:UTF-16", "", 1)
