# What the interpreter itself costs: a loop, a call, a block, an index — each
# 200,000 times, with nothing in the body worth measuring (#299).
#
#   target/release/spinel run bench/interp_loops.rb
#   ruby --yjit bench/interp_loops.rb
#
# Numbers go in the pull request, not here (`docs/roadmap.md`, rule 7).

def time(label)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started) * 1000
  puts "#{label}: #{elapsed.round(1)} ms"
end

N = 200_000

def noop; end
def one(x) = x

def yielder
  i = 0
  while i < N
    yield i
    i += 1
  end
end

a = Array.new(N, 1)
s = "abc"
time("while                ") { i = 0; i += 1 while i < N }
time("call, no arguments   ") { i = 0; while i < N; noop; i += 1; end }
time("call, one argument   ") { i = 0; while i < N; one(i); i += 1; end }
time("yield                ") { yielder { |x| } }
time("Array#each           ") { a.each { |x| } }
time("Integer#times        ") { N.times { |x| } }
time("a[i]                 ") { i = 0; while i < N; a[i]; i += 1; end }
time("String#size          ") { i = 0; while i < N; s.size; i += 1; end }
time("instance variable    ") { @v = 0; i = 0; while i < N; @v = i; i += 1; end }
time("Proc#call            ") { b = proc { |x| x }; i = 0; while i < N; b.call(i); i += 1; end }
time("a String literal     ") { i = 0; while i < N; "x"; i += 1; end }
time("allocate and drop    ") { i = 0; while i < N; [i, "x"]; i += 1; end }
