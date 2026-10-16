#!/usr/bin/env bash
# A census of a process's threads by name, once a second, on stdout:
#
#   16:34:50 threads=148 rss=4523MB vidcut-play-vid=32 vidcut-tiles-1=31 vidcut=9 ...
#
# A thread FFmpeg starts carries the name of the thread that started it, so
# vidcut-play-vid=32 is the player's decode thread and the 31 FFmpeg started
# from it. Linux keeps 15 characters of a name (vidcut-play-video shows as
# vidcut-play-vid); a space in one shows as _. Names are sorted by count.
#
# Usage: scripts/threads.sh [PID] [SECONDS]
#   PID: the process; by default the newest one named vidcut when the script
#   starts. SECONDS: how long; by default until the process ends or Ctrl + C.
#   Linux only: it reads /proc.
set -u

pid=${1:-$(pgrep -n -x vidcut)}
if [ -z "${pid:-}" ] || [ ! -d "/proc/$pid/task" ]; then
    echo "no such process${1:+: $1}; is vidcut running?" >&2
    exit 1
fi
seconds=${2:-}
n=0
while [ -d "/proc/$pid/task" ]; do
    names=$(cat /proc/"$pid"/task/*/comm 2>/dev/null | tr ' ' '_' | sort | uniq -c \
        | sort -k1,1nr -k2,2 | awk '{printf " %s=%s", $2, $1}')
    total=$(find /proc/"$pid"/task -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    rss=$(awk '/^VmRSS:/ {print int($2 / 1024)}' /proc/"$pid"/status 2>/dev/null)
    echo "$(date +%T) threads=$total rss=${rss:-0}MB$names"
    n=$((n + 1))
    if [ -n "$seconds" ] && [ "$n" -ge "$seconds" ]; then
        break
    fi
    sleep 1
done
