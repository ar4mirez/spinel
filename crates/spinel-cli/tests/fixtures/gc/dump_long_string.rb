# A String longer than the regexp engine will search in one go is dumped
# like any other: `ObjectSpace`'s JSON escaping does not search it with one
# regexp, which refused past 5,000,000 steps, about one a character.
require 'objspace'
long = "a" * 5_000_001
p ObjectSpace.dump(long).include?(%("bytesize":5000001, "value":"aaaa))
long << "\"\x7f"
p ObjectSpace.dump(long).include?(%(aaa\\"\\u007f"))
# Quotes and backslashes but no control character: escaped, the text is
# searched again for one, to its end, a step or more a character.
p ObjectSpace.dump('\"x' * 900_000).include?(%(\\\\\\"x\\\\\\"x"))
