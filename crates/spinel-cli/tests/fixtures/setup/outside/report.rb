# Outside any project: the environment is the one `spinel` was given.
puts "GEM_HOME=#{ENV["GEM_HOME"].inspect}"
puts "GEM_PATH=#{ENV["GEM_PATH"].inspect}"
puts "PATH=#{ENV["PATH"]}"
