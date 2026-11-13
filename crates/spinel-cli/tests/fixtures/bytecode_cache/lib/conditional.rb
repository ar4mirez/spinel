module Conditional
  def self.value
    if (x = 1) then x end
    if x = 2 then end
    x
  end
end
