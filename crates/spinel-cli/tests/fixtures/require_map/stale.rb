# Run after alpha/lib and beta/lib changed since the require map listed
# them: every answer must be the one a run without the map gives.
WHICH = []
alpha = File.join(__dir__, "alpha/lib")
beta = File.join(__dir__, "beta/lib")

$LOAD_PATH.unshift(alpha, beta)
require "added"
require "shadow"
$LOAD_PATH.unshift(beta)
require "gone"
begin
  require "only_alpha"
rescue LoadError => e
  puts e.message
end
puts WHICH.join(", ")
