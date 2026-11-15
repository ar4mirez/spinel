# One descriptor asked about twice in one poll(2): macOS's poll keeps one
# registration per descriptor, so the second entry used to replace the
# first, and the readiness only the first asked about never ended the wait.

# The same reader in the read and the error sets, its bytes there already.
r, w = IO.pipe
w.write("x")
p IO.select([r], nil, [r]).map(&:size)

# The same reader in the read and the error sets, its bytes written later.
writer = Thread.new { sleep 0.2; w.write("y") }
p IO.select([r], nil, [r]).map(&:size)
writer.join
r.read(2)

# Two threads waiting on one pipe, the scheduler polling it for both.
r2, w2 = IO.pipe
waiters = 2.times.map { |i| Thread.new { IO.select([r2]); i } }
sleep 0.2
w2.write("z")
p waiters.map(&:value)
