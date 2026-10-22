# The warning comes before the change (#341): a Warning.warn that raises
# leaves the chilled String without the method, the body unrun, no @x.
module Warning; def self.warn(*) = raise("boom"); end
s = "y"
begin; def s.foo; end; rescue => e; puts e.message; end
p s.singleton_methods
u = "u"
begin; class << u; puts "body ran"; end; rescue => e; puts e.message; end
class String; def setx; @x = 1; end; end
w = "w"
begin; w.setx; rescue => e; puts e.message; end
p w.instance_variable_get(:@x)
