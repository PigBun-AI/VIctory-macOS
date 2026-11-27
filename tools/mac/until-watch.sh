#!/bin/bash
# The "stop as soon as you have the data" watcher of tools/mac/run.sh (docs/mac/RUN.md, "Stop as soon as you have the data").
# run.sh starts it next to the title; it is not meant to be run by hand. It reads <out>/stdout.log and <out>/stderr.log (and,
# for captures, the capture folders) once per poll and ends when ANY condition holds:
#
#   until-watch.sh --out DIR --pid PID --t0 EPOCH [--log ERE]... [--world N] [--present N] [--capture ITEMS] [--record]
#                  [--linger SEC] [--final]
#
#   --log ERE      a line of stdout.log / stderr.log matches the extended regular expression (repeatable)
#   --world N      the log line `world frame N` (VICTORY_LOG_WORLD=1 makes the title log one per world frame)
#   --present N    the title's present number is N or more: the first column of <out>/frametime.log (--frametime-log) and the
#                  `frame=<n>` of the `frame presented` lines (powers of two only)
#   --capture L    every frame of the VICTORY_CAPTURE_FRAMES list L (present numbers and w<N>) is written: its
#                  `frame saved ...frame-<n>.png` line is in the log (the title logs it after the write), or the PNG is in
#                  $VICTORY_CAPTURE with the same size for 2 s
#   --record       the frame recorder has captured its frame: `frame recorder: the frame is captured` in the log, or
#                  notes.txt (written last) in a new $VICTORY_RECORD/capture-<n>; it counts together with --capture
#   --linger SEC   keep running SEC more seconds once a condition holds (the PNGs are written, the log gets more lines)
#   --final        no loop: one look (the title has ended; a file counts as stable at once)
#
# Exit status: 0 a condition holds (<out>/until-met.txt: "<which>\t<seconds>\t<line>"), 1 none did and the title is gone,
# 3 it can never hold (the recorder failed: <out>/until-failed.txt). The poll is 1 s (VICTORY_MAC_UNTIL_POLL, a test knob).
# Bash 3.2 (the Mac's /bin/bash): no associative arrays.
set -u
out=""; pid=""; t0=""; world=""; present=""; capture=""; record=0; linger=0; final=0; logs=()
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out=${2:?}; shift ;; --pid) pid=${2:?}; shift ;; --t0) t0=${2:?}; shift ;;
        --log) logs+=("${2:?}"); shift ;; --world) world=${2:?}; shift ;; --present) present=${2:?}; shift ;;
        --capture) capture=${2:?}; shift ;; --record) record=1 ;; --linger) linger=${2:?}; shift ;; --final) final=1 ;;
        *) echo "until-watch.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
[ -n "$out" ] && [ -n "$pid" ] && [ -n "$t0" ] || { echo "until-watch.sh: --out, --pid and --t0 are needed" >&2; exit 2; }
poll="${VICTORY_MAC_UNTIL_POLL:-1}"
OUTLOG="$out/stdout.log"; ERRLOG="$out/stderr.log"
# a Wine path (Z:\Users\...) as a Unix path; a Unix path as it is
unixpath() {
    case "$1" in
        [A-Za-z]:\\*) printf '%s' "${1#??}" | tr '\\' / ;;
        *) printf '%s' "$1" ;;
    esac
}
cap_dir=""; [ -z "${VICTORY_CAPTURE:-}" ] || cap_dir="$(unixpath "$VICTORY_CAPTURE")"
rec_dir=""; [ -z "${VICTORY_RECORD:-}" ] || rec_dir="$(unixpath "$VICTORY_RECORD")"
# the first match of an ERE in the two logs (the line, cut), or nothing
first_match() { grep -ah -m1 -E -e "$1" "$OUTLOG" "$ERRLOG" 2>/dev/null | head -n 1 | cut -c1-200; }
# the capture items: present numbers (cap_n) or w<N> (cap_w; the present is world frame 1's plus N), and what is done
cap_item=(); cap_kind=(); cap_val=(); cap_done=(); cap_size=(); cap_since=()
if [ -n "$capture" ]; then
    oldifs=$IFS; IFS=,
    for raw in $capture; do
        case "$raw" in [wW]*) k=w; v=${raw#[wW]} ;; *) k=p; v=$raw ;; esac
        cap_item+=("$raw"); cap_kind+=("$k"); cap_val+=("$v"); cap_done+=(0); cap_size+=(-1); cap_since+=(0)
    done
    IFS=$oldifs
fi
rec_done=0
fail() { printf '%s\n' "$1" >"$out/until-failed.txt"; exit 3; }

# one look at everything: sets MET_WHICH / MET_LINE and returns 0 when some condition holds
check() {
    local re m n i k v p first now sz f all newer cur g
    for re in ${logs[@]+"${logs[@]}"}; do
        m=$(first_match "$re")
        [ -z "$m" ] || { MET_WHICH="--until-log '$re'"; MET_LINE="$m"; return 0; }
    done
    if [ -n "$world" ]; then
        m=$(first_match "world frame ${world}\\b")
        [ -z "$m" ] || { MET_WHICH="--until-world $world"; MET_LINE="$m"; return 0; }
    fi
    if [ -n "$present" ]; then
        cur=0
        if [ -f "$out/frametime.log" ]; then
            f=$(tail -n 1 "$out/frametime.log" 2>/dev/null | cut -d, -f1)
            case "$f" in ''|*[!0-9]*) ;; *) cur=$f ;; esac
        fi
        g=$(grep -ahoE 'frame presented .*frame=[0-9]+' "$OUTLOG" "$ERRLOG" 2>/dev/null | sed 's/.*frame=//' | tail -n 1)
        case "$g" in ''|*[!0-9]*) ;; *) [ "$g" -le "$cur" ] || cur=$g ;; esac
        if [ "$cur" -ge "$present" ]; then
            MET_WHICH="--until-present $present"; MET_LINE="present $cur"; return 0
        fi
    fi
    if [ -n "$capture" ] || [ "$record" = 1 ]; then
        now=$(date +%s)
        all=1
        first=""
        i=0
        while [ "$i" -lt "${#cap_item[@]}" ]; do
            if [ "${cap_done[$i]}" = 0 ]; then
                k=${cap_kind[$i]}; v=${cap_val[$i]}
                if [ "$k" = w ]; then
                    if [ -z "$first" ]; then
                        first=$(first_match 'world frame 1 .*present=[0-9]+' | sed 's/.*present=\([0-9]*\).*/\1/')
                    fi
                    p=""; [ -z "$first" ] || p=$((first + v))
                else
                    p=$v
                fi
                if [ -n "$p" ]; then
                    if [ -n "$(first_match "[\\\\/]frame-${p}\\.png")" ]; then
                        cap_done[$i]=1
                    elif [ -n "$cap_dir" ] && [ -f "$cap_dir/frame-$p.png" ]; then
                        sz=$(stat -f %z "$cap_dir/frame-$p.png" 2>/dev/null || echo -1)
                        if [ "$sz" -gt 0 ] && [ "$final" = 1 ]; then cap_done[$i]=1
                        elif [ "$sz" != "${cap_size[$i]}" ]; then cap_size[$i]=$sz; cap_since[$i]=$now
                        elif [ "$sz" -gt 0 ] && [ $((now - cap_since[$i])) -ge 2 ]; then cap_done[$i]=1
                        fi
                    fi
                fi
            fi
            [ "${cap_done[$i]}" = 1 ] || all=0
            i=$((i + 1))
        done
        if [ "$record" = 1 ] && [ "$rec_done" = 0 ]; then
            if [ -n "$(first_match 'frame recorder: the frame is captured')" ]; then rec_done=1
            elif [ -n "$rec_dir" ] && [ -d "$rec_dir" ]; then
                newer=$(find "$rec_dir" -maxdepth 2 -name notes.txt -path '*/capture-*' -newer "$out/start.txt" 2>/dev/null | head -n 1)
                [ -z "$newer" ] || rec_done=1
            fi
            if [ "$rec_done" = 0 ]; then
                [ -z "$(first_match 'frame recorder not started')" ] || fail "the frame recorder did not start (see 'frame recorder not started' in the log)"
                if [ -n "$rec_dir" ] && [ -d "$rec_dir" ]; then
                    newer=$(find "$rec_dir" -maxdepth 2 -name failed.txt -path '*/capture-*' -newer "$out/start.txt" 2>/dev/null | head -n 1)
                    [ -z "$newer" ] || fail "the frame recorder failed ($newer)"
                fi
            fi
            [ "$rec_done" = 1 ] || all=0
        fi
        if [ "$all" = 1 ]; then
            MET_WHICH="--until-capture"
            [ -z "$capture" ] || MET_WHICH="$MET_WHICH (frames $capture"
            if [ "$record" = 1 ]; then
                [ -n "$capture" ] && MET_WHICH="$MET_WHICH, recording captured" || MET_WHICH="$MET_WHICH (recording captured"
            fi
            MET_WHICH="$MET_WHICH)"
            MET_LINE=""
            return 0
        fi
    fi
    return 1
}

MET_WHICH=""; MET_LINE=""
while :; do
    alive=1; kill -0 "$pid" 2>/dev/null || alive=0
    [ "$alive" = 1 ] || final=1
    if check; then
        printf '%s\t%s\t%s\n' "$MET_WHICH" "$(( $(date +%s) - t0 ))" "$MET_LINE" >"$out/until-met.txt"
        # the title may go on for LINGER more seconds (it ends sooner if it ends by itself)
        end=$(( $(date +%s) + linger ))
        while [ "$(date +%s)" -lt "$end" ] && kill -0 "$pid" 2>/dev/null; do sleep 0.5; done
        exit 0
    fi
    [ "$alive" = 1 ] && [ "$final" = 0 ] || exit 1
    sleep "$poll"
done
