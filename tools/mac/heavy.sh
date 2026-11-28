#!/bin/bash
# The one way to run a heavy job (build, test, census, probe, batch) on this Mac: a machine-wide budget of "job units" shared by
# every checkout, every worktree and every hand. Nothing else limits the machine (the verbal "at most 4 jobs, nice 19" did not hold).
#
#   tools/mac/heavy.sh [--jobs K] [--label TEXT] [--timeout SEC] [--run-timeout SEC] -- COMMAND [ARGS...]
#   tools/mac/heavy.sh run [same options] -- COMMAND ...        (the same)
#   tools/mac/heavy.sh status                                   slots, queue, paused jobs, budget, load, game lock
#   tools/mac/heavy.sh path                                     the state directory
#
#   --jobs K         job units wanted (default 1): the command gets CARGO_BUILD_JOBS=K, RAYON_NUM_THREADS=K, MAKEFLAGS=-jK,
#                    VICTORY_HEAVY_JOBS=K (and RUST_TEST_THREADS=max(K,2)). K is cut to the budget in force when it starts
#   --label TEXT     what to show in `status` (default: the command's name)
#   --timeout SEC    give up WAITING in the queue after SEC seconds (exit 4); default: wait for ever
#   --run-timeout SEC  stop the command (TERM, then KILL) after SEC seconds of running (exit 124); time spent paused does not count
#
# Rules (docs/mac/RUN.md "Heavy tasks"). The limit exists to keep frame times valid while a game runs; ordinary parallel work is not
# squeezed:
#  * Budget: with no game running VICTORY_HEAVY_BUDGET units (default: the number of cores, sysctl -n hw.ncpu) for all jobs of the
#    machine together. A job starts when the units in use plus its
#    own K fit into the budget; otherwise it queues (first come, first served; the queue is printed every 30 s). A job that asks for
#    more than the whole budget, or more than the budget in force while a game runs, runs with K cut to that budget, and only when
#    nothing else is running, so it can always start.
#  * A game running (the game lock tools/mac/game-lock.sh is held): the budget drops to 1 whatever the setting. Jobs already running are looked after by
#    their own heavy.sh (every 5 s, VICTORY_HEAVY_POLL): the one that started first keeps running, the others get SIGSTOP (their
#    process group; state "paused" in the slot file) and SIGCONT when the lock is released. A paused job still counts in the budget.
#  * The command runs as `nice -n 19`, in a process group of its own, in the foreground of this script, with its exit status.
#    On normal end, interrupt (INT/TERM/HUP) or --run-timeout the slot is removed and the group gets SIGCONT (an interrupt or time
#    limit also TERM, after 5 s KILL), so nothing stays stopped.
#  * Nested: a process that already is in a slot (VICTORY_HEAVY_SLOT set, its slot file there) just runs the command; no second slot,
#    so check.sh -> cargo -> build script ... cannot deadlock.
#  * The game itself (tools/mac/run.sh) does not go through here: it takes the game lock.
#
# State: $VICTORY_HEAVY_DIR (default ~/Documents/GTA6Project/research/.heavy)/ slots/<id> (one file per job: id owner ownerstart pid
# pidstart pgid jobs label cwd start startts state cmd), queue/<ticket>, lock.d (mkdir lock, taken for milliseconds only). A slot
# whose heavy.sh and command are both gone is removed by the next one who looks. No flock(1) needed (macOS has none).
# Also a library for tools/mac/heavy-guard.sh: `. tools/mac/heavy.sh` defines the hv_* functions and runs nothing.
# Environment (tests): VICTORY_HEAVY_DIR, VICTORY_HEAVY_BUDGET, VICTORY_HEAVY_POLL (5), VICTORY_HEAVY_QUEUE_POLL (1),
# VICTORY_GAME_LOCK (the game lock, as for game-lock.sh).

HV_SELF="${BASH_SOURCE[0]}"
HV_HERE="$(cd "$(dirname "$HV_SELF")" && pwd)"
HV_DIR="${VICTORY_HEAVY_DIR:-$HOME/Documents/GTA6Project/research/.heavy}"
HV_SLOTS="$HV_DIR/slots"
HV_QUEUE="$HV_DIR/queue"
HV_LOCKD="$HV_DIR/lock.d"

# the game lock: its place is VICTORY_GAME_LOCK, else the one in tools/mac/run.local.env (as game-lock.sh's own command line does)
if [ -z "${VICTORY_GAME_LOCK:-}" ] && [ -f "$HV_HERE/run.local.env" ]; then
    _l=$(sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}VICTORY_GAME_LOCK=\(.*\)$/\2/p' "$HV_HERE/run.local.env" | tail -1)
    [ -z "$_l" ] || eval "VICTORY_GAME_LOCK=$_l"
    unset _l
fi
. "$HV_HERE/game-lock.sh"

hv_say() { echo "heavy: $*" >&2; }
hv_game_held() { gl_state; [ $? = 0 ]; }
hv_int() { case "${1:-}" in ''|*[!0-9]*) return 1 ;; *) return 0 ;; esac; }
hv_ncpu() { local n; n=$(sysctl -n hw.ncpu 2>/dev/null); hv_int "$n" && [ "$n" -ge 1 ] || n=4; echo "$n"; }
# the budget with no game running: VICTORY_HEAVY_BUDGET, else the number of cores (sysctl -n hw.ncpu)
hv_base_budget() { local b="${VICTORY_HEAVY_BUDGET:-}"; hv_int "$b" || b=$(hv_ncpu); [ "$b" -ge 1 ] || b=1; echo "$b"; }
hv_budget() { if hv_game_held; then echo 1; else hv_base_budget; fi; }
hv_ticket() { perl -MTime::HiRes=time -e 'printf "%017.6f", time' 2>/dev/null || date +%s; }
hv_load() { sysctl -n vm.loadavg 2>/dev/null | tr -d '{}' | sed 's/^ *//; s/ *$//'; }

# ---- the lock: a directory made with mkdir (atomic); a dead holder's lock is moved away with an atomic rename ----
HV_LOCKED=0
hv_lock() {
    local n=0 p tmp
    mkdir -p "$HV_DIR" "$HV_SLOTS" "$HV_QUEUE" 2>/dev/null
    while ! mkdir "$HV_LOCKD" 2>/dev/null; do
        p=$(cat "$HV_LOCKD/pid" 2>/dev/null)
        if { [ -n "$p" ] && ! kill -0 "$p" 2>/dev/null; } || { [ -z "$p" ] && [ "$(gl_age "$HV_LOCKD")" -gt 10 ]; }; then
            tmp="$HV_LOCKD.stale.$$"
            if mv "$HV_LOCKD" "$tmp" 2>/dev/null; then
                p=$(cat "$tmp/pid" 2>/dev/null)
                if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then mv "$tmp" "$HV_LOCKD" 2>/dev/null   # raced with a live holder: put it back
                else rm -rf "$tmp"; fi
            fi
        fi
        n=$((n + 1)); [ "$n" -lt 2400 ] || { hv_say "the state lock $HV_LOCKD is stuck (held by pid ${p:-?})"; return 1; }
        sleep 0.05
    done
    echo "$$" >"$HV_LOCKD/pid"
    HV_LOCKED=1
}
hv_unlock() { [ "$HV_LOCKED" = 1 ] || return 0; HV_LOCKED=0; rm -rf "$HV_LOCKD"; }

# ---- slot files: key=value lines; S_* hold the slot loaded last ----
hv_slot_load() { # FILE -> S_*; fails if the file is not there
    S_id=""; S_owner=""; S_ownerstart=""; S_pid=""; S_pidstart=""; S_pgid=""; S_jobs=0; S_label=""; S_cwd=""; S_start=""; S_startts=0; S_state=""; S_cmd=""
    [ -f "$1" ] || return 1
    local k v
    while IFS== read -r k v; do
        case "$k" in
            id) S_id=$v ;; owner) S_owner=$v ;; ownerstart) S_ownerstart=$v ;; pid) S_pid=$v ;; pidstart) S_pidstart=$v ;;
            pgid) S_pgid=$v ;; jobs) S_jobs=$v ;; label) S_label=$v ;; cwd) S_cwd=$v ;; start) S_start=$v ;; startts) S_startts=$v ;;
            state) S_state=$v ;; cmd) S_cmd=$v ;;
        esac
    done <"$1" 2>/dev/null
    [ -n "$S_id" ]
}
hv_slot_write() { # FILE <- S_* (atomic)
    local tmp="$1.tmp.$$"
    {
        printf 'id=%s\nowner=%s\nownerstart=%s\npid=%s\npidstart=%s\npgid=%s\njobs=%s\nlabel=%s\ncwd=%s\nstart=%s\nstartts=%s\nstate=%s\ncmd=%s\n' \
            "$S_id" "$S_owner" "$S_ownerstart" "$S_pid" "$S_pidstart" "$S_pgid" "$S_jobs" "$S_label" "$S_cwd" "$S_start" "$S_startts" "$S_state" "$S_cmd"
    } >"$tmp" 2>/dev/null && mv "$tmp" "$1"
}
# a slot is alive while its heavy.sh or its command is
hv_slot_alive() {
    gl_alive "$S_owner" "$S_ownerstart" && return 0
    [ -n "$S_pid" ] && [ "$S_pid" != 0 ] && gl_alive "$S_pid" "$S_pidstart" && return 0
    return 1
}
# remove the slots of the dead (a group left paused by a heavy.sh that was killed gets SIGCONT); call with the lock held
hv_reap() {
    local f
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        hv_slot_load "$f" || { rm -f "$f"; continue; }
        if ! hv_slot_alive; then
            [ -z "$S_pgid" ] || kill -CONT -- "-$S_pgid" 2>/dev/null
            rm -f "$f"
        elif [ "$S_state" = paused ] && ! gl_alive "$S_owner" "$S_ownerstart" && [ -n "$S_pgid" ]; then
            # its heavy.sh is gone (killed), so nobody would ever resume it
            kill -CONT -- "-$S_pgid" 2>/dev/null; S_state=running; hv_slot_write "$f"
        fi
    done
}
hv_used() { # units in use by live slots (no reaping)
    local f t=0
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        hv_slot_load "$f" && hv_slot_alive && t=$((t + S_jobs))
    done
    echo "$t"
}
hv_first_live_slot() { # the id of the live slot that started first
    local f
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        if hv_slot_load "$f" && hv_slot_alive; then echo "$S_id"; return 0; fi
    done
    return 1
}
hv_live_pgids() { # "pgid pgid ..." and "pid pid ..." of the live slots (for the guard): sets HV_PGIDS HV_PIDS
    local f
    HV_PGIDS=" "; HV_PIDS=" "
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        if hv_slot_load "$f" && hv_slot_alive; then
            [ -z "$S_pgid" ] || HV_PGIDS="$HV_PGIDS$S_pgid "
            HV_PIDS="$HV_PIDS$S_owner $S_pid "
        fi
    done
}
# the queue: one file per waiter (ticket-pid): owner, ownerstart, jobs, label
hv_waiters() { # live waiters, first in line first; dead ones are removed
    local f p
    for f in $(ls "$HV_QUEUE" 2>/dev/null | LC_ALL=C sort); do
        p=$(sed -n 1p "$HV_QUEUE/$f" 2>/dev/null)
        if gl_alive "$p" "$(sed -n 2p "$HV_QUEUE/$f" 2>/dev/null)"; then echo "$f"; else rm -f "$HV_QUEUE/$f"; fi
    done
}

# ---- status ----
hv_status() {
    local f budget base held n=0 used=0 g w
    base=$(hv_base_budget); budget=$(hv_budget); hv_game_held && held=held || held=free
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        hv_slot_load "$f" && hv_slot_alive && used=$((used + S_jobs))
    done
    echo "heavy: budget $budget of $base units (game lock: $held$([ "$held" = held ] && echo ', budget cut to 1'))  in use $used  load $(hv_load)"
    echo "  state directory $HV_DIR"
    echo "slots:"
    for f in "$HV_SLOTS"/*; do
        [ -f "$f" ] || continue
        case "$f" in *.tmp.*) continue ;; esac
        hv_slot_load "$f" || continue
        n=$((n + 1))
        local st=$S_state; hv_slot_alive || st="$st(dead, the next job reclaims it)"
        printf '  %-28s pid %-6s pgid %-6s jobs %s  %-8s up %ss  [%s]  %s  (cwd %s)\n' "$S_id" "$S_pid" "$S_pgid" "$S_jobs" "$st" "$(( $(date +%s) - S_startts ))" "$S_label" "$S_cmd" "$S_cwd" | cut -c1-260
    done
    [ "$n" -gt 0 ] || echo "  (none)"
    echo "queue:"
    n=0
    for w in $(hv_waiters); do
        n=$((n + 1))
        printf '  #%s pid %s jobs %s [%s] waiting %ss\n' "$n" "$(sed -n 1p "$HV_QUEUE/$w")" "$(sed -n 3p "$HV_QUEUE/$w")" "$(sed -n 4p "$HV_QUEUE/$w")" "$(gl_age "$HV_QUEUE/$w")"
    done
    [ "$n" -gt 0 ] || echo "  (nobody is waiting)"
    gl_status | sed 's/^/  /'
}

# ---- run ----
hv_usage() { sed -n '2,/^$/p' "$HV_SELF" | sed '$d' | sed 's/^# \{0,1\}//' >&2; }

HV_SLOTFILE=""; HV_TICKETFILE=""; HV_CPID=""; HV_PGID=""; HV_CLEANED=0; HV_DONE=0
hv_group_signal() { [ -z "$HV_PGID" ] || kill "-$1" -- "-$HV_PGID" 2>/dev/null; }
hv_stop_group() { # CONT, TERM, wait up to 5 s, KILL
    local i
    hv_group_signal CONT; hv_group_signal TERM
    for i in 1 2 3 4 5 6 7 8 9 10; do
        { [ -n "$HV_CPID" ] && kill -0 "$HV_CPID" 2>/dev/null; } || break
        sleep 0.5
    done
    hv_group_signal KILL
}
hv_cleanup() {
    [ "$HV_CLEANED" = 0 ] || return 0
    HV_CLEANED=1
    if [ -n "$HV_CPID" ] && kill -0 "$HV_CPID" 2>/dev/null; then hv_stop_group; else hv_group_signal CONT; fi
    [ -z "$HV_SLOTFILE" ] || rm -f "$HV_SLOTFILE" "$HV_SLOTFILE".tmp.* 2>/dev/null
    [ -z "$HV_TICKETFILE" ] || rm -f "$HV_TICKETFILE" 2>/dev/null
    hv_unlock
}

hv_run() { # K LABEL QUEUE_TIMEOUT RUN_TIMEOUT -- COMMAND...
    local K=$1 label=$2 qtmo=$3 rtmo=$4; shift 4
    local base ticket me_start t0=$SECONDS last=0 first budget used keff pos reported=0 rc
    local pollq="${VICTORY_HEAVY_QUEUE_POLL:-1}" poll="${VICTORY_HEAVY_POLL:-5}"
    hv_int "$poll" || poll=5

    # nested: already in a slot
    if [ -n "${VICTORY_HEAVY_SLOT:-}" ] && [ -f "$HV_SLOTS/$VICTORY_HEAVY_SLOT" ]; then exec "$@"; fi
    [ -z "${VICTORY_HEAVY_SLOT:-}" ] || hv_say "VICTORY_HEAVY_SLOT=$VICTORY_HEAVY_SLOT names no slot of $HV_DIR (stale environment?): taking a slot of its own"
    command -v "$1" >/dev/null 2>&1 || [ -x "$1" ] || { hv_say "$1: command not found"; return 127; }

    base=$(hv_base_budget)
    [ "$K" -le "$base" ] || { hv_say "--jobs $K is more than the whole budget ($base): using $base"; K=$base; }
    trap 'hv_cleanup; exit 130' INT
    trap 'hv_cleanup; exit 143' TERM
    trap 'hv_cleanup; exit 129' HUP
    trap 'hv_cleanup' EXIT
    ticket="$(hv_ticket)-$$"
    me_start=$(gl_pidstart $$)
    hv_lock || return 1
    HV_TICKETFILE="$HV_QUEUE/$ticket"
    printf '%s\n%s\n%s\n%s\n' "$$" "$me_start" "$K" "$label" >"$HV_TICKETFILE"
    hv_unlock

    while :; do
        hv_lock || return 1
        hv_reap
        first=$(hv_waiters | sed -n 1p)
        if [ "$first" = "$ticket" ]; then
            budget=$(hv_budget); used=$(hv_used)
            keff=$K; [ "$keff" -le "$budget" ] || keff=$budget
            if [ $((used + K)) -le "$budget" ] || [ "$used" = 0 ]; then
                S_id="$ticket"; S_owner=$$; S_ownerstart="$me_start"; S_pid=""; S_pidstart=""; S_pgid=""; S_jobs=$keff
                S_label=$(printf '%s' "$label" | tr '\n\t' '  '); S_cwd=$PWD; S_start=$(date +%FT%T); S_startts=$(date +%s); S_state=starting
                S_cmd=$(printf '%s' "$*" | tr '\n' ' ' | cut -c1-200)
                HV_SLOTFILE="$HV_SLOTS/$ticket"
                hv_slot_write "$HV_SLOTFILE"
                rm -f "$HV_TICKETFILE"; HV_TICKETFILE=""
                hv_unlock
                break
            fi
        fi
        hv_unlock
        if [ $((SECONDS - last)) -ge 30 ] || [ "$reported" = 0 ]; then
            last=$SECONDS; reported=1
            pos=$(hv_waiters | grep -n -x -F "$ticket" | cut -d: -f1)
            hv_say "queued (place ${pos:-?}; wants $K of budget $(hv_budget)$(hv_game_held && echo ' [a game is running]'), $(hv_used) in use; $(hv_waiters | wc -l | tr -d ' ') waiting) [$label]"
        fi
        if [ "$qtmo" -gt 0 ] && [ $((SECONDS - t0)) -ge "$qtmo" ]; then
            hv_say "gave up waiting after ${qtmo} s [$label]"
            hv_cleanup; trap - EXIT; return 4
        fi
        sleep "$pollq"
    done
    keff=$S_jobs
    [ "$keff" = "$K" ] || hv_say "running with ${keff} job(s) instead of $K (budget $budget$(hv_game_held && echo ', a game is running'))"
    [ "$last" = 0 ] && [ "$reported" = 0 ] || hv_say "started after $((SECONDS - t0)) s: slot $ticket, $keff job(s) [$label]"

    # start: its own process group (perl), niced, the limits in the environment
    export VICTORY_HEAVY_SLOT="$ticket" VICTORY_HEAVY_JOBS="$keff" VICTORY_HEAVY_DIR="$HV_DIR"
    export CARGO_BUILD_JOBS="$keff" RAYON_NUM_THREADS="$keff" MAKEFLAGS="-j$keff"
    export RUST_TEST_THREADS="$([ "$keff" -ge 2 ] && echo "$keff" || echo 2)"
    local launch=(perl -e 'setpgrp(0,0); exec @ARGV; print STDERR "heavy: cannot run $ARGV[1]: $!\n"; exit 127' nice -n 19 "$@")
    if [ -t 0 ]; then "${launch[@]}" </dev/null & else "${launch[@]}" <&0 & fi
    HV_CPID=$!
    local i; for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
        [ "$(ps -o pgid= -p "$HV_CPID" 2>/dev/null | tr -d ' ')" = "$HV_CPID" ] && break
        kill -0 "$HV_CPID" 2>/dev/null || break
        sleep 0.1
    done
    HV_PGID="$HV_CPID"
    S_pid="$HV_CPID"; S_pidstart=$(gl_pidstart "$HV_CPID"); S_pgid="$HV_PGID"; S_state=running
    hv_slot_write "$HV_SLOTFILE"

    # supervision: one look at the game lock every $poll seconds
    local nextchk=$SECONDS running_s=0 lastsec=$SECONDS paused=0 mine
    while kill -0 "$HV_CPID" 2>/dev/null; do
        sleep 0.5
        [ "$paused" = 1 ] || running_s=$((running_s + SECONDS - lastsec))
        lastsec=$SECONDS
        if [ "$rtmo" -gt 0 ] && [ "$running_s" -ge "$rtmo" ]; then
            hv_say "run time limit ${rtmo} s reached: stopping [$label]"
            hv_stop_group; wait "$HV_CPID" 2>/dev/null; rc=124; HV_CPID=""
            hv_cleanup; trap - EXIT; return 124
        fi
        [ "$SECONDS" -ge "$nextchk" ] || continue
        nextchk=$((SECONDS + poll))
        if hv_game_held; then
            mine=0; [ "$(hv_first_live_slot)" = "$ticket" ] && mine=1
            if [ "$mine" = 0 ] && [ "$paused" = 0 ]; then
                hv_group_signal STOP; paused=1; S_state=paused; hv_slot_write "$HV_SLOTFILE"
                hv_say "a game is running: paused (SIGSTOP) until the lock is released [$label]"
            elif [ "$mine" = 1 ] && [ "$paused" = 1 ]; then
                hv_group_signal CONT; paused=0; S_state=running; hv_slot_write "$HV_SLOTFILE"
                hv_say "a game is running and this is the first job: resumed [$label]"
            fi
        elif [ "$paused" = 1 ]; then
            hv_group_signal CONT; paused=0; S_state=running; hv_slot_write "$HV_SLOTFILE"
            hv_say "the game lock is released: resumed (SIGCONT) [$label]"
        fi
    done
    wait "$HV_CPID" 2>/dev/null; rc=$?
    HV_CPID=""
    hv_cleanup; trap - EXIT
    return "$rc"
}

# ---- command line (not when sourced) ------------------------------------------
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    set -u
    case "${1:-}" in
        status) hv_status; exit 0 ;;
        path) echo "$HV_DIR"; exit 0 ;;
        -h|--help|help) hv_usage; exit 0 ;;
        run) shift ;;
    esac
    jobs_k=1; label=""; qtmo=0; rtmo=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --jobs) hv_int "${2:-}" && [ "$2" -ge 1 ] || { hv_say "--jobs needs a number >= 1"; exit 2; }; jobs_k=$2; shift ;;
            --label) label=${2:?--label TEXT}; shift ;;
            --timeout) hv_int "${2:-}" || { hv_say "--timeout needs seconds"; exit 2; }; qtmo=$2; shift ;;
            --run-timeout) hv_int "${2:-}" || { hv_say "--run-timeout needs seconds"; exit 2; }; rtmo=$2; shift ;;
            --) shift; break ;;
            -*) hv_say "unknown option $1"; hv_usage; exit 2 ;;
            *) break ;;
        esac
        shift
    done
    [ $# -gt 0 ] || { hv_usage; exit 2; }
    [ -n "$label" ] || label=$(basename "$1")
    hv_run "$jobs_k" "$label" "$qtmo" "$rtmo" "$@"
    exit $?
fi
