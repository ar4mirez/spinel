# Byte escapes past ASCII in a UTF-8 pattern spell one character, as Onigmo
# reads them; Rack::Builder strips a BOM with /\A\xef\xbb\xbf/.
def try(source)
  r = Regexp.new(source)
  p [source, r.encoding, "a\u{feff}b" =~ r, "xあy" =~ r]
rescue RegexpError => e
  p [source, e.class, e.message]
end

try('\xef\xbb\xbf')
try('\A\xef\xbb\xbf')
try('\xe3\x81\x82')
try('[\xe3\x81\x82]')
try('\343\201\202')
try('\xff')
try('\xe3\x81')
try('\xc3')
try('[\x80-\xff]')
p(/\xe3\x81\x82/ =~ "xあ")
s = +"\u{feff}hello"
s.slice!(/\A#{'\xef\xbb\xbf'}/)
p s
# A match walking a long subject once is answered.
p(("x" * 3_000_000 + "tail") =~ /x+tail\z/)
