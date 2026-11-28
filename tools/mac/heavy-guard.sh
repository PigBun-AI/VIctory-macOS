#!/bin/bash
# The safety net under tools/mac/heavy.sh: the verbal rule "go through heavy.sh" does not bind every hand, so this watches the machine
# WHILE A GAME RUNS (the game lock, tools/mac/game-lock.sh, is held) and quiets the heavy processes that are not in a heavy.sh slot.
# With the game lock free it does nothing (parallel work is not limited then), except to let go of what it stopped earlier.
#
#   tools/mac/heavy-guard.sh run          the loop (what launchd runs): a scan every VICTORY_GUARD_INTERVAL seconds (default 10)
#   tools/mac/heavy-guard.sh once         one scan, then exit (tests; what it stopped stays stopped, `once` again with the lock free resumes it)
#   tools/mac/heavy-guard.sh install      write ~/Library/LaunchAgents/com.gta6project.heavy-guard.plist and start it (refuses if an agent
#                                         of that label is already loaded; --dry-run prints the plist and does nothing)
#                                         (exit 3: loaded but macOS keeps launchd's /bin/bash out of ~/Documents until it has Full Disk Access)
#   tools/mac/heavy-guard.sh uninstall    stop it, remove the plist (what it had stopped is resumed first)
#   tools/mac/heavy-guard.sh status       the agent's state (launchctl), the mode, the stopped list, the last log lines
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
# VICTORY_GAME_LOCK, VICTORY_HEAVY_DIR as for heavy.sh, VICTORY_GUARD_SCOPE_PID=<pid> limits a scan to the descendants of that process.
# launchd: label com.gta6project.heavy-guard (VICTORY_GUARD_LABEL), KeepAlive; output in <state dir>/guard.out.

GUARD_SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
. "$(dirname "$GUARD_SELF")/heavy.sh"

GUARD_LABEL="${VICTORY_GUARD_LABEL:-com.gta6project.heavy-guard}"
GUARD_PLIST_DIR="${VICTORY_GUARD_PLIST_DIR:-$HOME/Library/LaunchAgents}"
GUARD_LOG="${VICTORY_GUARD_LOG:-$HV_DIR/guard.log}"
GUARD_STOPPED="$HV_DIR/guard.stopped"
GUARD_MODE="$HV_DIR/guard.mode"

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

gd_run() {
    local cycle=0
    trap 'gd_resume; exit 0' TERM INT HUP
    trap 'gd_resume' EXIT
    echo "$(date +%FT%T) heavy-guard started (pid $$, interval ${VICTORY_GUARD_INTERVAL:-10} s, lock $GL_LOCK)" >>"$GUARD_LOG"
    while :; do
        gd_scan
        cycle=$((cycle + 1))
        if [ $((cycle % 360)) = 0 ] && [ "$(stat -f %z "$GUARD_LOG" 2>/dev/null || echo 0)" -gt 2000000 ]; then mv -f "$GUARD_LOG" "$GUARD_LOG.1"; fi
        sleep "${VICTORY_GUARD_INTERVAL:-10}" &
        wait $!
    done
}

# ---- launchd ----
gd_plist_path() { echo "$GUARD_PLIST_DIR/$GUARD_LABEL.plist"; }
gd_xml() { sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g'; }
gd_plist() {
    local v
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$GUARD_LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>$(printf '%s' "$GUARD_SELF" | gd_xml)</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ThrottleInterval</key><integer>10</integer>
    <key>ProcessType</key><string>Background</string>
    <key>Nice</key><integer>5</integer>
    <key>StandardOutPath</key><string>$(printf '%s' "$HV_DIR/guard.out" | gd_xml)</string>
    <key>StandardErrorPath</key><string>$(printf '%s' "$HV_DIR/guard.out" | gd_xml)</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key><string>$(printf '%s' "$HOME" | gd_xml)</string>
        <key>PATH</key><string>/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:$(printf '%s' "$HOME" | gd_xml)/.local/bin</string>
EOF
    for v in VICTORY_HEAVY_DIR VICTORY_GAME_LOCK VICTORY_HEAVY_BUDGET VICTORY_GUARD_NAMES VICTORY_GUARD_ARGS VICTORY_GUARD_PROTECT \
             VICTORY_GUARD_INTERVAL VICTORY_GUARD_STOP_LOAD VICTORY_GUARD_RESUME_LOAD VICTORY_GUARD_LOG; do
        [ -z "${!v:-}" ] || printf '        <key>%s</key><string>%s</string>\n' "$v" "$(printf '%s' "${!v}" | gd_xml)"
    done
    cat <<EOF
    </dict>
</dict>
</plist>
EOF
}
gd_loaded() { launchctl list "$GUARD_LABEL" >/dev/null 2>&1; }
gd_agent_pid() { launchctl list "$GUARD_LABEL" 2>/dev/null | sed -n 's/^[[:space:]]*"PID" = \([0-9]*\);.*/\1/p'; }

gd_install() {
    local dry=0 plist uid pid n
    [ "${1:-}" != --dry-run ] || dry=1
    plist=$(gd_plist_path); uid=$(id -u)
    if [ $dry = 1 ]; then gd_plist; return 0; fi
    if gd_loaded; then
        echo "heavy-guard: an agent $GUARD_LABEL is already loaded (pid $(gd_agent_pid)); not installing over it. Run '$0 uninstall' first." >&2
        return 1
    fi
    n=$(pgrep -f "heavy-guard.sh run" | grep -v "^$$\$" | tr '\n' ' ')
    [ -z "$n" ] || { echo "heavy-guard: a 'heavy-guard.sh run' is already running by hand (pid $n); stop it first." >&2; return 1; }
    mkdir -p "$GUARD_PLIST_DIR" "$HV_DIR" || return 1
    gd_plist >"$plist" || return 1
    plutil -lint "$plist" >/dev/null || { echo "heavy-guard: the plist is not valid" >&2; return 1; }
    launchctl bootstrap "gui/$uid" "$plist" 2>/dev/null || launchctl load -w "$plist" || { echo "heavy-guard: launchctl could not load $plist" >&2; return 1; }
    sleep 3
    if gd_loaded; then
        pid=$(gd_agent_pid)
        echo "heavy-guard: installed $plist; launchctl list: label $GUARD_LABEL pid ${pid:-?}; log $GUARD_LOG"
        # macOS (TCC) keeps a launchd job's /bin/bash out of ~/Documents (the checkout, the game lock and this state directory are there) until
        # /bin/bash has Full Disk Access: then the job dies at once with "Operation not permitted" (status 126) and launchd retries every 10 s
        if [ -z "$pid" ] && tail -n 3 "$HV_DIR/guard.out" 2>/dev/null | grep -q "Operation not permitted"; then
            echo "heavy-guard: BUT the agent cannot run: macOS does not let a launchd job read ~/Documents (\"Operation not permitted\", see $HV_DIR/guard.out)." >&2
            echo "  Fix (once, by the user): System Settings > Privacy & Security > Full Disk Access > + > Cmd-Shift-G /bin/bash > switch it on;" >&2
            echo "  launchd retries every 10 s, so the guard then starts by itself. Until then '$0 run' from a terminal/app that may read ~/Documents works (not restarted by launchd)." >&2
            return 3
        fi
    else
        echo "heavy-guard: loaded, but launchctl list does not show $GUARD_LABEL" >&2; return 1
    fi
}
gd_uninstall() {
    local plist uid
    plist=$(gd_plist_path); uid=$(id -u)
    if gd_loaded; then launchctl bootout "gui/$uid/$GUARD_LABEL" 2>/dev/null || launchctl unload "$plist" 2>/dev/null; sleep 1; fi
    gd_loaded && { echo "heavy-guard: $GUARD_LABEL is still loaded" >&2; return 1; }
    rm -f "$plist"
    GD_LOAD=$(gd_load); GD_HELD=uninstall; gd_resume
    echo "heavy-guard: uninstalled $GUARD_LABEL"
}
gd_status() {
    if gd_loaded; then echo "agent $GUARD_LABEL: loaded, pid $(gd_agent_pid)"; else echo "agent $GUARD_LABEL: not loaded"; fi
    echo "plist: $(gd_plist_path) $([ -f "$(gd_plist_path)" ] && echo present || echo absent)"
    echo "mode: $([ "$(cat "$GUARD_MODE" 2>/dev/null)" = stop ] && echo 'stopping (game running, load high)' || echo normal)   stopped now: $(wc -l <"$GUARD_STOPPED" 2>/dev/null | tr -d ' ' || echo 0)"
    echo "log $GUARD_LOG:"; tail -n 8 "$GUARD_LOG" 2>/dev/null | sed 's/^/  /'
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case "${1:-}" in
        run) gd_run ;;
        once) GD_LOAD=0; GD_HELD=?; gd_scan ;;
        install) shift; gd_install "$@" ;;
        uninstall) gd_uninstall ;;
        status) gd_status ;;
        *) sed -n '2,/^$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
    esac
fi
