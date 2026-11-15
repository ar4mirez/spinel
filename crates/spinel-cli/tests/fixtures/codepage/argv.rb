# ARGV is in the default external encoding, after `-E`; `$0` the locale's (#41).
p ARGV.map { [_1.encoding, _1.frozen?] }, $0.encoding
