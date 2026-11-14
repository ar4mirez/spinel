# Both threads are part-way through their autoloads before either refers to
# the other's constant.
$in_x << true
$in_y.pop
class CycleX
  CycleY
end
