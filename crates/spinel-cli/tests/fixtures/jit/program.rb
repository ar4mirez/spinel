# Run by `tests/jit.rs` with and without `SPINEL_JIT=force`: the two runs must
# print the same. Each method takes a different way out of compiled code.
def fib(n) = n < 2 ? n : fib(n - 1) + fib(n - 2)

def count(n)
  i = 0
  s = 0
  while i < n
    s += i
    i += 1
  end
  s
end

def inc(a) = a + 1
def cmp(a, b) = [a < b, a <= b, a > b, a >= b, a == b, a != b]
def opt(a, b = a + 1) = [a, b]

p fib(18)
p count(50_000)
p inc(4611686018427387903)
p cmp(1, 2), cmp(2, 2), cmp("a", "b")
p opt(1), opt(1, 9)
p [1, 2, 3].map { |x| inc(x) }
p(begin
  inc(nil)
rescue NoMethodError => e
  e.message
end)
p Thread.new { count(30_000) }.value
