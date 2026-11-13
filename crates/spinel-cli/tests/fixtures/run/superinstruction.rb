# `--dump-bytecode` shows a superinstruction (#746): `i < 3` and `i += 1` are a
# local, an integer and an operator, each one instruction, followed by the
# instructions it stands for.
i = 0
while i < 3
  i += 1
end
puts i
