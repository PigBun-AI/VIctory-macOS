#!/bin/bash
# The safety net under tools/mac/heavy.sh: the verbal rule "go through heavy.sh" does not bind every hand, so this watches the machine
# WHILE A GAME RUNS (the game lock, tools/mac/game-lock.sh, is held) and quiets the heavy processes that are not in a heavy.sh slot.
# With the game lock free it does nothing (parallel work is not limited then), except to let go of what it stopped earlier.
#
#   tools/mac/heavy-guard.sh run [--parent PID]
#                                         the loop: a scan every VICTORY_GUARD_INTERVAL seconds (default 10). tools/mac/run.sh starts it as its
#                                         own child, `run --parent <run.sh's pid>`, once it holds the game lock, and ends it when it lets the
#                                         lock go: the guard lives and dies with the game run (no launchd agent: macOS does not let a launchd
#                                         job's bash read ~/Documents). Started by hand without --parent it runs until it is stopped.
#   tools/mac/heavy-guard.sh once         one scan, then exit (tests; what it stopped stays stopped, `once` again with the lock free resumes it)
#   tools/mac/heavy-guard.sh resume       send SIGCONT to everything this guard stopped (the list is <state dir>/guard.stopped), then exit
#   tools/mac/heavy-guard.sh running      exit 0 if a guard is running (<state dir>/guard.pid, or a `heavy-guard.sh run` by hand), else 1
#   tools/mac/heavy-guard.sh status       the mode, the guard running, the stopped list, the last log lines
#   tools/mac/heavy-guard.sh install|uninstall   gone: the guard is run.sh's child now ("已改由 run.sh 托管"); both say so and exit 2
#
# With --parent (the hosted guard) it ends by itself, after SIGCONT of everything it stopped, when
#  * the parent process is gone (checked every second, kill -0 and not a zombie, same start time): run.sh was killed with SIGKILL, so no
#    orphan is left, and
#  * the game lock is not held any more (the lock was let go, or was never taken).
#
# What a scan does (one `ps`, one awk, a few tests):
#  * lock free: nothing (a process it stopped before is sent SIGCONT).
#  * lock held: every heavy process outside a heavy.sh slot is reniced to 19 (once). Heavy = the first word of its command line (or,
#    under wine, its .exe) matches VICTORY_GUARD_NAMES (default cargo rustc rustdoc victory-census victory-shader-exec probe-wine.exe
#    clang clang++ cc1 cc1plus ld ld64 ld64.lld lld rust-lld x86_64-w64-mingw32-clang(++)), or the whole command line matches
#    VICTORY_GUARD_ARGS (default: a cargo test binary, target/*/deps/<name>-<hex hash>).
#  * lock held and the 1-minute load average > VICTORY_GUARD_STOP_LOAD (30): the same processes also get SIGSTOP. They get SIGCONT when
#    the lock is released or the load is back under VICTORY_GUARD_RESUME_LOAD (20) (between the two the state stays as it was).
#  * In a slot = its process group is the group of a live slot of heavy.sh, or one of its parents is that slot's heavy.sh or command.
#  * Never touched: the game (victory.exe ... run ... --execute), wineserver, winedevice, anything with run.sh in its command line and
#    everything below a run.sh or below the process that holds the game lock, heavy.sh and this script (VICTORY_GUARD_PROTECT).
#  * Only SIGSTOP, SIGCONT and renice, only for this user's processes, never a kill. A process is stopped at most once and only a process
#    this guard stopped (same pid, same start time) is ever sent SIGCONT; when the guard itself ends, it resumes them.
# Every action is a line of <state dir>/guard.log: time, pid, command, action, load, lock. Settings can also be in <state dir>/guard.conf
# (shell assignments of the variables above, read at every scan). Tests: VICTORY_GUARD_LOAD=<number> replaces the load average,
# VICTORY_GAME_LOCK, VICTORY_HEAVY_DIR as for heavy.sh, VICTORY_GUARD_SCOPE_PID=<pid> limits a scan to the descendants of that process,
# VICTORY_GUARD_PATTERN is the pgrep pattern `running` uses to see a guard started by hand. Output of a hosted guard: <state dir>/guard.out.

GUARD_SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
. "$(dirname "$GUARD_SELF")/heavy.sh"

GUARD_LOG="${VICTORY_GUARD_LOG:-$HV_DIR/guard.log}"
GUARD_STOPPED="$HV_DIR/guard.stopped"
GUARD_MODE="$HV_DIR/guard.mode"
GUARD_PID="$HV_DIR/guard.pid"
GD_PARENT=""; GD_PARENT_START=""

gd_log() { # PID CMD ACTION
    printf '%s pid=%s cmd="%s" action=%s load=%s lock=%s\n' "$(date +%FT%T)" "$1" "$(printf '%s' "$2" | cut -c1-120)" "$3" "$GD_LOAD" "$GD_HELD" >>"$GUARD_LOG"
}
gd_conf() { [ ! -f "$HV_DIR/guard.conf" ] || . "$HV_DIR/guard.conf"; }
gd_load() { # the 1-minute load average
    if [ -n "${VICTORY_GUARD_LOAD:-}" ]; then echo "$VICTORY_GUARD_LOAD"; else sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}'; fi
}
gd_num_gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 > b + 0) }'; }
gd_num_lt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 < b + 0) }'; }

# resume what this guard stopped (same pid and start time only)
gd_resume() {
    local p s
    [ -s "$GUARD_STOPPED" ] || return 0
    while IFS=$'\t' read -r p s c; do
        [ -n "$p" ] || continue
        if gl_alive "$p" "$s"; then kill -CONT "$p" 2>/dev/null; gd_log "$p" "$c" "SIGCONT (stopped earlier by the guard)"; fi
    done <"$GUARD_STOPPED"
    : >"$GUARD_STOPPED"
}

gd_scan() {
    local names args protect holder mode="" line pid ni stat cmd stop_load resume_load pgids pids
    gd_conf
    names="${VICTORY_GUARD_NAMES:-^(cargo|rustc|rustdoc|victory-census|victory-shader-exec|probe-wine\.exe|clang|clang\+\+|cc1|cc1plus|ld|ld64|ld64\.lld|lld|rust-lld|x86_64-w64-mingw32-clang|x86_64-w64-mingw32-clang\+\+)\$}"
    args="${VICTORY_GUARD_ARGS-/target/[^ ]*/deps/[A-Za-z0-9_]+-[0-9a-f]+( |\$)}"
    protect="${VICTORY_GUARD_PROTECT:-victory\.exe.* run .*--execute|wineserver|winedevice|run\.sh|heavy-guard|heavy\.sh}"
    stop_load="${VICTORY_GUARD_STOP_LOAD:-30}"; resume_load="${VICTORY_GUARD_RESUME_LOAD:-20}"
    mkdir -p "$HV_DIR" 2>/dev/null
    GD_LOAD=$(gd_load)
    if ! hv_game_held; then
        GD_HELD=free; rm -f "$GUARD_MODE"
        gd_resume
        return 0
    fi
    GD_HELD=held
    # stop mode: on above the stop load, off below the resume load, in between as it was
    [ ! -f "$GUARD_MODE" ] || mode=$(cat "$GUARD_MODE" 2>/dev/null)
    if gd_num_gt "$GD_LOAD" "$stop_load"; then mode=stop
    elif gd_num_lt "$GD_LOAD" "$resume_load"; then mode=""; fi
    printf '%s\n' "$mode" >"$GUARD_MODE"
    [ "$mode" = stop ] || gd_resume

    holder=$(cat "$GL_LOCK/pid" 2>/dev/null)
    hv_live_pgids; pgids=$HV_PGIDS; pids=$HV_PIDS
    while IFS=$'\t' read -r pid ni stat cmd; do
        [ -n "$pid" ] || continue
        if [ "${ni:-0}" -lt 19 ] 2>/dev/null && renice -n 19 -p "$pid" >/dev/null 2>&1; then gd_log "$pid" "$cmd" "renice 19 (was $ni)"; fi
        if [ "$mode" = stop ]; then
            case "$stat" in *T*) continue ;; esac
            if kill -STOP "$pid" 2>/dev/null; then
                printf '%s\t%s\t%s\n' "$pid" "$(gl_pidstart "$pid")" "$cmd" >>"$GUARD_STOPPED"
                gd_log "$pid" "$cmd" SIGSTOP
            fi
        fi
    done < <(ps -U "$(id -u)" -ww -o pid=,ppid=,pgid=,ni=,stat=,args= 2>/dev/null | GD_NAMES="$names" GD_ARGS="$args" GD_PROTECT="$protect" GD_SCOPE="${VICTORY_GUARD_SCOPE_PID:-0}" \
        awk -v holder="${holder:-0}" -v me="$$" -v slotpgids="$pgids" -v slotpids="$pids" '
        BEGIN { names = ENVIRON["GD_NAMES"]; argsre = ENVIRON["GD_ARGS"]; protect = ENVIRON["GD_PROTECT"]; scope = ENVIRON["GD_SCOPE"] }
        function base(s,  n, a) { n = split(s, a, "/"); return a[n] }
        {
            p = $1; pp[p] = $2; pg[p] = $3; ni[p] = $4; st[p] = $5
            c = ""; for (i = 6; i <= NF; i++) c = c (i > 6 ? " " : "") $i
            cmd[p] = c; order[++n] = p
        }
        function runsh(p,  w) { split(cmd[p], w, " "); return (w[1] ~ /(^|\/)run\.sh$/ || w[2] ~ /(^|\/)run\.sh$/) }
        function belowrun(p,  d) { # the process or a parent is run.sh
            for (d = 0; p > 1 && d < 40; d++) { if (runsh(p)) return 1; p = pp[p] }
            return 0
        }
        function below(p, q,  d) { # q is p or one of its parents
            if (q + 0 <= 1) return 0
            for (d = 0; p > 1 && d < 40; d++) { if (p == q) return 1; p = pp[p] }
            return 0
        }
        function slotted(p,  d) {
            if (index(slotpgids, " " pg[p] " ")) return 1
            for (d = 0; p > 1 && d < 40; d++) { if (index(slotpids, " " p " ")) return 1; p = pp[p] }
            return 0
        }
        END {
            for (k = 1; k <= n; k++) {
                p = order[k]
                if (p == me) continue
                split(cmd[p], w, " "); b = base(w[1]); hit = (b ~ names)
                if (!hit && b ~ /^wine/) for (j = 2; j in w; j++) if (tolower(w[j]) ~ /\.exe$/) { hit = (base(w[j]) ~ names); break }
                if (!hit && argsre != "" && cmd[p] ~ argsre) hit = 1
                if (!hit) continue
                if (cmd[p] ~ protect || belowrun(p) || below(p, holder) || slotted(p)) continue
                if (scope + 0 > 0 && !below(p, scope)) continue
                printf "%s\t%s\t%s\t%s\n", p, ni[p], st[p], substr(cmd[p], 1, 160)
            }
        }')
}

# ---- the hosted guard: lives and dies with its parent (run.sh) ----
# the parent is alive: kill -0, not a zombie (a killed run.sh whose own parent has not waited for it yet), and the same start time
gd_parent_alive() {
    local st rest
    kill -0 "$GD_PARENT" 2>/dev/null || return 1
    read -r st rest < <(ps -o stat=,lstart= -p "$GD_PARENT" 2>/dev/null)
    case "$st" in ''|Z*) return 1 ;; esac
    [ -z "$GD_PARENT_START" ] || [ "$rest" = "$GD_PARENT_START" ]
}
# <state dir>/guard.pid: pid, start time, parent (empty when started by hand)
gd_pidfile_write() { printf '%s\n%s\n%s\n' "$$" "$(gl_pidstart $$)" "$GD_PARENT" >"$GUARD_PID.tmp.$$" && mv -f "$GUARD_PID.tmp.$$" "$GUARD_PID"; }
gd_pidfile_drop() { [ "$(sed -n 1p "$GUARD_PID" 2>/dev/null)" = "$$" ] && rm -f "$GUARD_PID"; return 0; }
# a guard is running: the pid file names a live one (a hosted one only while its parent lives, as one that is about to end does not count),
# or a plain `heavy-guard.sh run` (no --parent) is there that predates the pid file
gd_running() {
    local p s par
    p=$(sed -n 1p "$GUARD_PID" 2>/dev/null); s=$(sed -n 2p "$GUARD_PID" 2>/dev/null); par=$(sed -n 3p "$GUARD_PID" 2>/dev/null)
    if [ -n "$p" ] && [ "$p" != "$$" ] && gl_alive "$p" "$s" && { [ -z "$par" ] || kill -0 "$par" 2>/dev/null; }; then return 0; fi
    pgrep -f "${VICTORY_GUARD_PATTERN:-^[^ ]*bash( -[a-z]+)* [^ ]*heavy-guard\\.sh run *\$}" 2>/dev/null | grep -v -x -F "$$" | grep -q .
}
# sleep one interval; a hosted guard looks at its parent every second
gd_nap() {
    local iv="${VICTORY_GUARD_INTERVAL:-10}" i=0
    case "$iv" in
        ''|*[!0-9]*) sleep "$iv" & wait $!; return 0 ;;
    esac
    if [ -z "$GD_PARENT" ]; then sleep "$iv" & wait $!; return 0; fi
    while [ "$i" -lt "$iv" ]; do
        sleep 1 & wait $!
        gd_parent_alive || return 0
        i=$((i + 1))
    done
}

gd_run() {
    local cycle=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --parent) GD_PARENT="${2:-}"; shift ;;
            *) echo "heavy-guard: run: unknown argument $1" >&2; exit 2 ;;
        esac
        shift
    done
    if [ -n "$GD_PARENT" ]; then
        case "$GD_PARENT" in *[!0-9]*) echo "heavy-guard: --parent takes a pid" >&2; exit 2 ;; esac
        GD_PARENT_START=$(gl_pidstart "$GD_PARENT")
    fi
    mkdir -p "$HV_DIR" 2>/dev/null
    [ -z "$GD_PARENT" ] || exec >>"$HV_DIR/guard.out" 2>&1
    trap 'gd_resume; gd_pidfile_drop; exit 0' TERM INT HUP
    trap 'gd_resume; gd_pidfile_drop' EXIT
    gd_pidfile_write
    echo "$(date +%FT%T) heavy-guard started (pid $$, interval ${VICTORY_GUARD_INTERVAL:-10} s, lock $GL_LOCK${GD_PARENT:+, parent $GD_PARENT})" >>"$GUARD_LOG"
    while :; do
        if [ -n "$GD_PARENT" ] && ! gd_parent_alive; then
            GD_LOAD=$(gd_load); GD_HELD=parent-gone; gd_resume
            echo "$(date +%FT%T) heavy-guard: parent $GD_PARENT is gone, resumed what it stopped, ending (pid $$)" >>"$GUARD_LOG"
            exit 0
        fi
        gd_scan
        if [ -n "$GD_PARENT" ] && [ "$GD_HELD" = free ]; then
            echo "$(date +%FT%T) heavy-guard: the game lock is not held, resumed what it stopped, ending (pid $$)" >>"$GUARD_LOG"
            exit 0
        fi
        cycle=$((cycle + 1))
        if [ $((cycle % 360)) = 0 ] && [ "$(stat -f %z "$GUARD_LOG" 2>/dev/null || echo 0)" -gt 2000000 ]; then mv -f "$GUARD_LOG" "$GUARD_LOG.1"; fi
        gd_nap
    done
}

gd_gone() {
    echo "heavy-guard: '$1' is gone: the guard is run.sh's child now (it starts it once it holds the game lock and ends it with the run); there is no launchd agent any more. 已改由 run.sh 托管。" >&2
    exit 2
}
gd_resume_cmd() { GD_LOAD=$(gd_load); GD_HELD=resume; gd_resume; echo "heavy-guard: resumed what it had stopped ($GUARD_STOPPED)"; }
gd_status() {
    local p par
    if gd_running; then
        p=$(sed -n 1p "$GUARD_PID" 2>/dev/null); par=$(sed -n 3p "$GUARD_PID" 2>/dev/null)
        echo "guard: running (pid ${p:-?}, ${par:+run.sh pid $par}${par:-started by hand or by an older version})"
    else echo "guard: not running"; fi
    echo "mode: $([ "$(cat "$GUARD_MODE" 2>/dev/null)" = stop ] && echo 'stopping (game running, load high)' || echo normal)   stopped now: $( [ -f "$GUARD_STOPPED" ] && wc -l <"$GUARD_STOPPED" | tr -d ' ' || echo 0)"
    echo "log $GUARD_LOG:"; tail -n 8 "$GUARD_LOG" 2>/dev/null | sed 's/^/  /'
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case "${1:-}" in
        run) shift; gd_run "$@" ;;
        once) GD_LOAD=0; GD_HELD=?; gd_scan ;;
        resume) gd_resume_cmd ;;
        running) gd_running ;;
        install|uninstall) gd_gone "$1" ;;
        status) gd_status ;;
        *) sed -n '2,/^$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
    esac
fi
