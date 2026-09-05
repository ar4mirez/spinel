# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
x = 1
case x
in Integer => n
  puts n
end
