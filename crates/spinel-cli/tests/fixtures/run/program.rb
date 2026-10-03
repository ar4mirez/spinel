# A program's own state: its arguments, its streams, and how it ends.
p ARGV
p File.basename($0)
$stdout.puts "to stdout"
$stderr.puts "to stderr"
p ENV.key?("PATH"), ENV["SPINEL_TEST_UNSET"]
at_exit { puts "at_exit runs last" }
at_exit { puts "and in reverse" }
exit 3
