# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
#
# It was `case`/`in` until #165 compiled it, a class variable until #188's
# fixtures needed them, and a backtick command until #145 compiled it to a
# call. A rational literal is next: it waits on `Rational` (#227).
p 3r
