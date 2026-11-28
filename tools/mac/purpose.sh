#!/bin/bash
# Library (sourced by run.sh, drop-ab.sh, rt-builder-accept.sh): the --purpose every game run must give.
#   --purpose "<map column>[,<map column>...]: <node id>[,<node id>...]"
#   map column: freq | callstats | new-nodes | corpus | perf-hotspot (the columns of the translation map a run is allowed to fill)
#   node id:    a node of the translation matrix (docs/mac/translation-matrix-mac.csv.txt, for example mac.isa.lock_cmpxchg_flags), at least one
# A run that has none, or one that does not fit, is refused with exit status PURPOSE_EXIT (7: no other exit status of run.sh is 7).
# Rule of the integration lead: until the map is filled in, no run is made to look at the picture.
PURPOSE_EXIT=7
_pc='(freq|callstats|new-nodes|corpus|perf-hotspot)'
_pid='[A-Za-z0-9][A-Za-z0-9._-]*'
_purpose_re="^[[:space:]]*${_pc}([[:space:]]*,[[:space:]]*${_pc})*[[:space:]]*:[[:space:]]*${_pid}([[:space:]]*,[[:space:]]*${_pid})*[[:space:]]*\$"
purpose_ok() { [[ "${1:-}" =~ $_purpose_re ]]; }
# purpose_check VALUE: prints why to stderr and returns PURPOSE_EXIT when it does not fit
purpose_check() {
    local v=${1:-}
    if [ -z "$v" ]; then
        echo "--purpose is required: --purpose \"<map column>: <node id>,...\", the column one of freq callstats new-nodes corpus perf-hotspot (several with commas), then at least one node id; for example --purpose \"freq: mac.isa.lock_cmpxchg_flags\" (exit status $PURPOSE_EXIT)" >&2
        return $PURPOSE_EXIT
    fi
    if ! purpose_ok "$v"; then
        case "$v" in
            *:*) if [[ "${v%%:*}" =~ ^[[:space:]]*${_pc}([[:space:]]*,[[:space:]]*${_pc})*[[:space:]]*$ ]]; then
                     echo "--purpose '$v': after the colon at least one node id is needed (letters, digits, . _ -, separated by commas)" >&2
                 else
                     echo "--purpose '$v': the map column must be one of freq callstats new-nodes corpus perf-hotspot (several separated by commas), not '${v%%:*}'" >&2
                 fi ;;
            *) echo "--purpose '$v': the form is \"<map column>: <node id>,...\" (no colon found)" >&2 ;;
        esac
        return $PURPOSE_EXIT
    fi
    return 0
}
