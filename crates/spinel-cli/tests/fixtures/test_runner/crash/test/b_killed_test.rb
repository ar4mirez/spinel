# Dies mid-file, as a segfault in a C extension would.
Process.kill(:KILL, Process.pid)
