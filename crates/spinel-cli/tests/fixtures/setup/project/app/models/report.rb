# What the setup shim left for the program and for the processes it starts.
puts "GEM_HOME=#{ENV["GEM_HOME"].inspect}"
puts "GEM_PATH=#{ENV["GEM_PATH"].inspect}"
puts "child PATH=#{`printenv PATH`.chomp}"
system("spinel-setup-fixture") or puts "no executable on PATH"
