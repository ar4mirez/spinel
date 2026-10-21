s = "a"
t = eval('"b"')
p [s.frozen?, (+s).equal?(s), (+t).equal?(t)]
s << "x" unless s.frozen?
t << "y" unless t.frozen?
