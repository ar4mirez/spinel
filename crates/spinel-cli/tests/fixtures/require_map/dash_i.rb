# Run with `-I alpha/lib`, which comes before the program's own entries.
WHICH = []
$LOAD_PATH.push(File.join(__dir__, "beta/lib"))
require "which"
require "nested/deep"
puts WHICH.join(", ")
