# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
#
# It was `case`/`in` until #165 compiled it, a class variable until #188's
# fixtures needed them, a backtick command until #145 compiled it to a call,
# and a rational and then a complex literal until those classes existed. A
# regexp standing alone as a condition is next: it matches against `$_`.
p(1) if /a/
