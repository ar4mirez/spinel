# What an `at_exit` block and a `trap(:EXIT)` handler see of the stack
# (#899): their own frames and nothing of the code that ends the program,
# as in CRuby, where that code is C. Minitest runs its tests from such a
# block, so this is what its failure backtraces end with.
def fails = raise("boom")

at_exit { fails }

at_exit do
  p caller
  p caller_locations.map(&:to_s)
  begin
    raise "inside"
  rescue => e
    p e.backtrace
    p e.backtrace_locations.map(&:to_s)
  end
end

trap(:EXIT) { p caller }
