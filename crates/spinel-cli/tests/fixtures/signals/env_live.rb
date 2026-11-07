# `ENV` is the process's environment, not a copy: a child sees what was set
# and does not see what was deleted (#44).
ENV["SPINEL_SIGNALS_ENV"] = "set"
print `sh -c 'echo "$SPINEL_SIGNALS_ENV"'`
ENV.delete("SPINEL_SIGNALS_ENV")
print `sh -c 'echo "[${SPINEL_SIGNALS_ENV-unset}]"'`
ENV.update("SPINEL_SIGNALS_A" => "a", "SPINEL_SIGNALS_B" => "b")
system("sh", "-c", 'echo "$SPINEL_SIGNALS_A$SPINEL_SIGNALS_B"')
p ENV.slice("SPINEL_SIGNALS_A", "SPINEL_SIGNALS_B")
