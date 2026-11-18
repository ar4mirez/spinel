# What reflection sees of an IO, a File and a File::Stat: nothing, as on
# CRuby, whose IO keeps its state in the T_FILE struct and File::Stat in a
# T_DATA one. Psych dumps an object by its instance variables, so a File
# dumps as an empty mapping. `state.out` is what ruby 4.0.9 printed.
require "yaml"

file = File.new(__FILE__)
reader, writer = IO.pipe

puts "file: #{file.instance_variables.inspect}"
puts "pipe reader: #{reader.instance_variables.inspect}"
puts "pipe writer: #{writer.instance_variables.inspect}"
puts "stdout: #{$stdout.instance_variables.inspect}"
puts "stat: #{File.stat(__FILE__).instance_variables.inspect}"
print YAML.dump(file)

# The state is still there, only out of sight.
writer.write("through the pipe")
writer.close
puts reader.read
puts file.gets
puts File.basename(file.path)

file.close
reader.close

# A File::Stat is a T_DATA on CRuby, with nothing Marshal can write.
begin
  Marshal.dump(File.stat(__FILE__))
rescue TypeError => e
  puts "Marshal File::Stat: #{e.class}: #{e.message}"
end
