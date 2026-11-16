# A program that changes `$LOAD_PATH` between requires. The require map
# lists alpha/lib and beta/lib; local/ is the program's own and is not in it.
# Every answer must be the one a run without the map gives.
WHICH = []
alpha = File.join(__dir__, "alpha/lib")
beta = File.join(__dir__, "beta/lib")
local = File.join(__dir__, "local")

$LOAD_PATH.unshift(beta, alpha)
require "which"
$LOAD_PATH.delete(beta)
load "which.rb"
$LOAD_PATH.unshift(local)
load "which.rb"
$LOAD_PATH.unshift(beta)
load "which.rb"
p require("which")
require "nested/deep"
require "only_alpha"
p $LOAD_PATH.resolve_feature_path("gone")
begin
  require "nested/none"
rescue LoadError => e
  puts e.message
end
puts WHICH.join(", ")
