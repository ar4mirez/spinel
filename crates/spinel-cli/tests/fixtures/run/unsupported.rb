# Valid Ruby that this build does not compile yet. When it does, this fixture
# has to change — which is the point: the test fails loudly rather than
# silently checking nothing.
#
# It was `case`/`in` until #165 compiled it, and a class variable until #188's
# fixtures needed them. A backtick command is the next construct with no meaning
# here at all: it needs a subprocess, which is phase 3.
`echo hi`
