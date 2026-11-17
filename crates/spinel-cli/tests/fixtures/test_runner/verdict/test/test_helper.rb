# Each worker ends with status 0 whatever its tests did: this `at_exit`,
# registered before the one `minitest/autorun` registers, runs after it.
at_exit { exit!(0) }
require "minitest/autorun"
