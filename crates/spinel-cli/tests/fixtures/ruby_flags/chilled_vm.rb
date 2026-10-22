# Each of these thaws a chilled String through a VM instruction, not a
# method (#341): one warning apiece, and none for the second change.
s = "y"; def s.foo; end; def s.bar; end
u = "v"; class << u; end; class << u; end
class String; def setx; @x = 1; end; end
w = "w"; w.setx; w.setx
p [s.frozen?, u.frozen?, w.frozen?]
