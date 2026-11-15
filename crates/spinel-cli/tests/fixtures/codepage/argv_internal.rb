# With a default internal encoding, ARGV is transcoded into it (#41).
p ARGV.map { [_1.encoding, _1.frozen?, _1.bytes] }
