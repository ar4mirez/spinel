# `objspace/trace` (#690): upstream's file, which starts tracing allocations
# and makes `p` say where each object it prints was made.
#
# This is only for debugging purpose. Do not use this in production.
# Require'ing this file immediately starts tracing the object allocation,
# which brings a large performance overhead.

require 'objspace'

module Kernel
  remove_method :p
  define_method(:p) do |*objs|
    objs.each do |obj|
      file = ObjectSpace.allocation_sourcefile(obj)
      line = ObjectSpace.allocation_sourceline(obj)
      if file
        puts "#{ obj.inspect } @ #{ file }:#{ line }"
      else
        puts obj.inspect
      end
    end
  end
end

ObjectSpace.trace_object_allocations_start

warn "objspace/trace is enabled"
