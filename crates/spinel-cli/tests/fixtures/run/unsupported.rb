# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
#
# It was `case`/`in` until #165 compiled it. A class variable is the next
# construct with no meaning here at all.
class C
  @@count = 0
end
