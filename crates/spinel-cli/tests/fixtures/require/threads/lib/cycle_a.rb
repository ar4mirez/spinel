# Both threads are part-way through their files before either requires the
# other's.
$in_a << true
$in_b.pop
require "cycle_b"
