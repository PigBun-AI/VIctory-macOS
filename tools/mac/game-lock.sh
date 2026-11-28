#!/bin/bash
# The one lock for running the game on this Mac: at any time only ONE victory.exe instance (two can run the GPU out of
# memory, and any heavy CPU job makes frame times meaningless). tools/mac/run.sh takes it before it starts the title or
# `--exec` runs victory.exe (not for --print-env); every checkout / worktree shares it.
#
#   tools/mac/game-lock.sh status     who holds the lock now, since when, and who is waiting (exit 0)
#   tools/mac/game-lock.sh held       exit 0 if a live process holds the lock, 1 if not (prints nothing)
#   tools/mac/game-lock.sh path       the lock directory
#
# Also a library: `. tools/mac/game-lock.sh` gives gl_acquire / gl_release (used by run.sh).
#
# How it works: the lock is a directory made with mkdir (atomic). In it: pid, pidstart (the process start time, so a
# reused pid is not mistaken for the holder), since, worktree, backend, out, cmd, purpose, and guard.pid / guard.pidstart (the heavy-guard run.sh
# started, see tools/mac/heavy-guard.sh). A lock whose pid is gone is stale and
# the next one who looks takes it over (moved away with an atomic rename, then removed). Waiters queue in
# <lock>.queue/ (one file each, ordered by arrival) and take the lock in that order; a waiter that died is dropped.
# Place: $VICTORY_GAME_LOCK, else $HOME/Documents/GTA6Project/research/.victory-game.lock (VICTORY_GAME_LOCK can be set
# in tools/mac/run.local.env). GL_POLL (seconds, default 1) is the polling interval, GL_REPORT (default 30) how often a
# waiter prints the holder.

GL_LOCK="${VICTORY_GAME_LOCK:-$HOME/Documents/GTA6Project/research/.victory-game.lock}"
GL_QUEUE="$GL_LOCK.queue"

gl_pidstart() { ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//'; }
# gl_alive PID START: the process exists and (when START is given) started at that time
gl_alive() {
    [ -n "${1:-}" ] || return 1
    kill -0 "$1" 2>/dev/null || return 1
    [ -z "${2:-}" ] || [ "$(gl_pidstart "$1")" = "$2" ]
}
gl_field() { cat "$1/$2" 2>/dev/null; } # gl_field DIR NAME
gl_age() { # seconds since the mtime of a path
    local m; m=$(stat -f %m "$1" 2>/dev/null) || { echo 0; return; }
    echo $(( $(date +%s) - m ))
}
# 0 = a live holder, 1 = no lock, 2 = a stale lock (the holder is gone)
gl_state() {
    [ -d "$GL_LOCK" ] || return 1
    local p; p=$(gl_field "$GL_LOCK" pid)
    if [ -z "$p" ]; then
        # being written right now, unless it has been like this for a while
        [ "$(gl_age "$GL_LOCK")" -lt 10 ] && return 0
        return 2
    fi
    gl_alive "$p" "$(gl_field "$GL_LOCK" pidstart)" && return 0
    return 2
}
gl_describe() { # one line about the holder in $1 (default: the lock)
    local d=${1:-$GL_LOCK}
    [ -d "$d" ] || { printf '(nobody: the lock was just released)'; return; }
    printf 'pid %s since %s backend %s worktree %s out %s%s' "$(gl_field "$d" pid)" "$(gl_field "$d" since)" \
        "$(gl_field "$d" backend)" "$(gl_field "$d" worktree)" "$(gl_field "$d" out)" "$([ -s "$d/purpose" ] && printf ' purpose "%s"' "$(gl_field "$d" purpose)")"
}
# take over a stale lock: rename it away (only one process can win), check what we moved, remove it
gl_reclaim() {
    local tmp="$GL_LOCK.stale.$$.$RANDOM" p
    mv "$GL_LOCK" "$tmp" 2>/dev/null || return 1
    p=$(gl_field "$tmp" pid)
    if gl_alive "$p" "$(gl_field "$tmp" pidstart)"; then
        # we raced with a new holder: put it back (if that fails the new holder will notice nothing; it is a rare race)
        mv "$tmp" "$GL_LOCK" 2>/dev/null
        return 1
    fi
    echo "game-lock: took over a stale lock ($(gl_describe "$tmp"); that process is gone)" >&2
    rm -f "$tmp"/* 2>/dev/null; rmdir "$tmp" 2>/dev/null
    return 0
}
gl_waiters() { # live waiters in arrival order, one entry file name per line; dead ones are dropped
    local f p
    [ -d "$GL_QUEUE" ] || return 0
    for f in $(ls "$GL_QUEUE" 2>/dev/null | LC_ALL=C sort); do
        p=$(sed -n 1p "$GL_QUEUE/$f" 2>/dev/null)
        if gl_alive "$p" "$(sed -n 2p "$GL_QUEUE/$f" 2>/dev/null)"; then echo "$f"; else rm -f "$GL_QUEUE/$f"; fi
    done
}
gl_ticket=""
gl_enqueue() { # gl_enqueue BACKEND OUT
    mkdir -p "$GL_QUEUE" 2>/dev/null || return 0
    local t; t=$(perl -MTime::HiRes=time -e 'printf "%017.6f", time' 2>/dev/null || date +%s)
    gl_ticket="$t-$$"
    printf '%s\n%s\n%s\n%s\n%s\n%s\n' "$$" "$(gl_pidstart $$)" "$(date +%FT%T)" "${GL_WORKTREE:-?}" "${1:-?}" "${2:-?}" >"$GL_QUEUE/$gl_ticket"
    printf '%s\n' "${GL_PURPOSE:-}" >>"$GL_QUEUE/$gl_ticket"
}
gl_dequeue() { [ -z "$gl_ticket" ] || rm -f "$GL_QUEUE/$gl_ticket"; gl_ticket=""; }

gl_owner=0 # 1 once this process holds the lock
# gl_acquire BACKEND OUT [TIMEOUT_SECONDS, 0 = wait for ever]  -> 0 got it, 1 timed out
gl_acquire() {
    local backend=${1:-?} out=${2:-?} tmo=${3:-0} start now last=0 first pos
    mkdir -p "$(dirname "$GL_LOCK")" 2>/dev/null
    start=$(date +%s)
    gl_enqueue "$backend" "$out"
    while :; do
        gl_state; case $? in 2) gl_reclaim ;; esac
        first=$(gl_waiters | sed -n 1p)
        if [ -z "$first" ] || [ "$first" = "$gl_ticket" ]; then
            if mkdir "$GL_LOCK" 2>/dev/null; then
                printf '%s\n' "$$" >"$GL_LOCK/pid.tmp" && mv "$GL_LOCK/pid.tmp" "$GL_LOCK/pid"
                gl_pidstart $$ >"$GL_LOCK/pidstart"
                date +%FT%T >"$GL_LOCK/since"
                printf '%s\n' "${GL_WORKTREE:-?}" >"$GL_LOCK/worktree"
                printf '%s\n' "$backend" >"$GL_LOCK/backend"
                printf '%s\n' "$out" >"$GL_LOCK/out"
                printf '%s\n' "${GL_CMD:-?}" >"$GL_LOCK/cmd"
                printf '%s\n' "${GL_PURPOSE:-}" >"$GL_LOCK/purpose"
                gl_owner=1; gl_dequeue
                return 0
            fi
        fi
        now=$(date +%s)
        if [ "$tmo" -gt 0 ] && [ $((now - start)) -ge "$tmo" ]; then
            gl_dequeue
            echo "game-lock: gave up after ${tmo} s; the game is still held by: $(gl_describe)" >&2
            return 1
        fi
        if [ $((now - last)) -ge "${GL_REPORT:-30}" ]; then
            last=$now
            pos=$(gl_waiters | grep -n -x -F "$gl_ticket" | cut -d: -f1)
            echo "game-lock: waiting (place ${pos:-?} in the queue, $(gl_waiters | wc -l | tr -d ' ') waiting); held by: $(gl_describe)" >&2
        fi
        sleep "${GL_POLL:-1}"
    done
}
# release: the queue entry always, the lock only if this process holds it
gl_release() {
    gl_dequeue
    [ "$gl_owner" = 1 ] || return 0
    gl_owner=0
    if [ "$(gl_field "$GL_LOCK" pid)" = "$$" ]; then
        rm -f "$GL_LOCK"/* 2>/dev/null; rmdir "$GL_LOCK" 2>/dev/null
    fi
    return 0
}

gl_status() {
    local f
    gl_state; case $? in
        0) echo "game lock: HELD  $(gl_describe)"
           echo "  purpose: $(gl_field "$GL_LOCK" purpose)"
           echo "  command: $(gl_field "$GL_LOCK" cmd)   (lock $GL_LOCK)" ;;
        2) echo "game lock: STALE (the holder is gone; the next run takes it over)  $(gl_describe)" ;;
        *) echo "game lock: free  (lock $GL_LOCK)" ;;
    esac
    local n=0
    for f in $(gl_waiters); do
        n=$((n + 1))
        echo "  waiting #$n: pid $(sed -n 1p "$GL_QUEUE/$f") since $(sed -n 3p "$GL_QUEUE/$f") backend $(sed -n 5p "$GL_QUEUE/$f") worktree $(sed -n 4p "$GL_QUEUE/$f") out $(sed -n 6p "$GL_QUEUE/$f") purpose \"$(sed -n 7p "$GL_QUEUE/$f")\""
    done
    [ "$n" -gt 0 ] || echo "  nobody is waiting"
}

# ---- command line (not when sourced) ------------------------------------------
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    if [ -z "${VICTORY_GAME_LOCK:-}" ]; then
        _root="$(cd "$(dirname "$0")/../.." && pwd)"
        if [ -f "$_root/tools/mac/run.local.env" ]; then
            # only the lock's place is wanted from the local settings
            _l=$(sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}VICTORY_GAME_LOCK=\(.*\)$/\2/p' "$_root/tools/mac/run.local.env" | tail -1)
            [ -z "$_l" ] || { eval "VICTORY_GAME_LOCK=$_l"; GL_LOCK="$VICTORY_GAME_LOCK"; GL_QUEUE="$GL_LOCK.queue"; }
        fi
    fi
    case "${1:-}" in
        status) gl_status ;;
        held) gl_state ;;
        path) echo "$GL_LOCK" ;;
        *) sed -n '2,/^$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
    esac
fi
