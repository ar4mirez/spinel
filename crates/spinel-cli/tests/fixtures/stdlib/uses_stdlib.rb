# Requires two vendored stdlib files, one of which requires others.
require "ipaddr"
require "optparse"

puts IPAddr.new("192.168.1.7/24").to_s
options = {}
OptionParser.new { |o| o.on("-v", "--verbose") { options[:verbose] = true } }.parse!(%w[-v])
p options
puts $LOAD_PATH.last == File.dirname($LOADED_FEATURES.grep(/ipaddr\.rb\z/).first)
