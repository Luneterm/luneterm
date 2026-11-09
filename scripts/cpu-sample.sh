#!/bin/sh
# Mean CPU use of a process over a window: scripts/cpu-sample.sh <pid> <secs>
#
# Prints one number: the process's CPU time used during the window divided by the window, as a
# percentage of one core (a process using two cores fully reads 200). It reads the process's
# accumulated CPU time with `ps -o time=` twice, <secs> apart, so it needs no root and works on
# macOS and Linux. Used for plan M10 ledger 16: the outline must cost at most 3 points more while a
# command runs than at an idle prompt:
#
#   scripts/cpu-sample.sh <pid> 30      # at an idle prompt
#   scripts/cpu-sample.sh <pid> 30      # during `sleep 60`
#
# Exit status: 0 and a number on stdout; 2 on a usage error or when the process is not there.
# The CPU time has the resolution `ps` gives (10 ms on macOS, 1 s on some Linux builds): use a
# window of 10 s or more.

set -eu

usage() {
    echo "usage: $0 <pid> <secs>" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
pid=$1
secs=$2
case "$pid" in '' | *[!0-9]*) usage ;; esac
case "$secs" in '' | *[!0-9]*) usage ;; esac
[ "$secs" -gt 0 ] || usage

# `[[dd-]hh:]mm:ss[.cc]` to seconds.
cpu_seconds() {
    t=$(ps -o time= -p "$1" 2>/dev/null | tr -d ' ') || return 1
    [ -n "$t" ] || return 1
    echo "$t" | awk '{
        s = $0; d = 0
        if (index(s, "-")) { split(s, a, "-"); d = a[1]; s = a[2] }
        n = split(s, p, ":")
        sec = 0
        for (i = 1; i <= n; i++) sec = sec * 60 + p[i]
        printf "%.2f\n", d * 86400 + sec
    }'
}

start=$(cpu_seconds "$pid") || { echo "no process $pid" >&2; exit 2; }
sleep "$secs"
end=$(cpu_seconds "$pid") || { echo "process $pid ended during the window" >&2; exit 2; }
awk -v a="$start" -v b="$end" -v w="$secs" 'BEGIN { printf "%.1f\n", (b - a) / w * 100 }'
