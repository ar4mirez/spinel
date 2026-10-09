# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
#
# It was `case`/`in` until #165 compiled it, a class variable until #188's
# fixtures needed them, a backtick command until #145 compiled it to a call,
# and a rational literal until `Rational` existed. A complex literal is next:
# it waits on `Complex` (#227).
p 3i
