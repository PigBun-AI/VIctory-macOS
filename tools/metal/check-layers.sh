#!/bin/sh
# Machine check of the layering rules (architecture note 2.5 and "分层规矩",
# docs/metal). Exit status 0 = clean.
#
#   proto   depends on nothing (core and alloc only); names no D3D12 / DXGI /
#           HRESULT / Metal / objc2 / IR... token                      (rules 3a, 4)
#   front   depends on neither objc2* nor back; names no Metal / objc2 / IR
#           token                                                      (rule 3c)
#   back    depends on no windows* crate; names no D3D12 token; uses no Metal 3
#           command object (MTLCommandQueue, MTLCommandBuffer, ...
#           encoders, useResource, useHeap, addCompletedHandler, setBytes) in
#           src/                                                       (rules 3b, 11, 2.5-6)
#   bridge  is the only crate with Wine symbols (__wine_*, winemac, macdrv) (rule 5)
#   upstream crates do not depend on victory-metal-*                   (rule 1)
#
# The one exemption: back/src/m0.rs keeps the Metal 3 queue on purpose. It is
# the M0 measurement that compares the classic queue with the Metal 4 queue
# (docs/metal/m0-results.md); without it the comparison could not be rerun. The
# exemption is for that file and for the Metal 3 rule only (its D3D12, Wine and
# dependency rules still apply); it ends when m0.rs is deleted. Nothing else
# is exempt, and a new file cannot opt out by a comment.
#
# Crates that do not exist (yet) are reported as absent and skipped; the rules
# apply to them from the day they appear. Dependencies are read from the
# manifests ([dependencies], [dev-/build-dependencies], target tables,
# [dependencies.NAME], `package = "..."` renames); a crate with none cannot
# have transitive ones, so no `cargo tree` is needed.
#
#   tools/metal/check-layers.sh               check metal/ of this checkout
#   tools/metal/check-layers.sh --self-test   prove each rule catches a violation
#
# METAL_DIR and UPSTREAM_DIRS override where it looks (the self-test uses them).
set -u
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)

D3D12='ID3D12|D3D12_|IDXGI|DXGI_'
METAL='(^|[^A-Za-z0-9_])MTL[A-Z0-9]|objc2|(^|[^A-Za-z0-9_])IR[A-Z]|CAMetal'
WINE='__wine_|winemac|macdrv'
# Architecture note 2.5 item 6: the Metal 3 command objects. `MTLCommandBuffer`
# must not be followed by an identifier character, so `MTLCommandBufferStatus`
# (an enum, not a command object) is not caught.
METAL3='MTLCommandQueue|MTLBlitCommandEncoder|useResource|useHeap|addCompletedHandler|setBytes|MTLComputeCommandEncoder|MTLRenderCommandEncoder|MTLParallelRenderCommandEncoder|MTLCommandBuffer([^A-Za-z0-9_]|$)'
# Dependency names (not source text).
DEP_WINDOWS='^windows([-_].*)?$'
DEP_OBJC2='^objc2([-_].*)?$'
DEP_BACK='^victory[-_]metal[-_]back$'
DEP_BACK_FORBIDDEN='^victory[-_]metal[-_](bridge|front)$'

# dep_names MANIFEST: one dependency name per line (the key, and the real name
# when it is renamed with `package`).
dep_names() {
    awk '
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    function emit(n) { n = trim(n); gsub(/^"|"$/, "", n); if (n != "") print n }
    /^[ \t]*#/ { next }
    /^[ \t]*\[/ {
        h = $0
        sub(/^[ \t]*\[+/, "", h)
        sub(/[ \t]*(#.*)?$/, "", h)
        sub(/\]+$/, "", h)
        mode = 0
        if (h ~ /(^|\.)(dev-|build-)?dependencies$/) {
            mode = 1
        } else if (match(h, /(^|\.)(dev-|build-)?dependencies\./)) {
            mode = 2
            emit(substr(h, RSTART + RLENGTH))
        }
        next
    }
    mode == 1 && /=/ {
        key = $0
        sub(/[ \t]*=.*/, "", key)
        sub(/\..*/, "", key)
        emit(key)
        if (match($0, /package[ \t]*=[ \t]*"[^"]+"/)) {
            p = substr($0, RSTART, RLENGTH)
            sub(/^[^"]*"/, "", p)
            sub(/"$/, "", p)
            emit(p)
        }
    }
    mode == 2 && /^[ \t]*package[ \t]*=/ {
        p = $0
        sub(/^[^"]*"/, "", p)
        sub(/".*$/, "", p)
        emit(p)
    }
    ' "$1"
}

check() {
    metal=${METAL_DIR:-$ROOT/metal}
    upstream=${UPSTREAM_DIRS:-"$ROOT/crates $ROOT/apps $ROOT/xtask $ROOT/Cargo.toml"}
    failed=0

    # files DIR: every Rust source and manifest below metal/DIR (not target/).
    files() {
        [ -d "$metal/$1" ] || return 0
        find "$metal/$1" \( -name target -o -name .git \) -prune -o \
            \( -name '*.rs' -o -name Cargo.toml \) -type f -print
    }

    report() { # LABEL HITS
        echo "FAIL  $1:"
        echo "$2" | sed 's/^/        /'
        failed=1
    }

    # forbid LABEL PATTERN CRATE...: no match of PATTERN in the sources and manifests.
    forbid() {
        label=$1
        pattern=$2
        shift 2
        for crate in "$@"; do
            list=$(files "$crate")
            [ -n "$list" ] || continue
            # shellcheck disable=SC2086
            hits=$(echo "$list" | tr '\n' '\0' | xargs -0 grep -nHE "$pattern" 2>/dev/null)
            [ -z "$hits" ] || report "$label ($crate)" "$hits"
        done
    }

    # forbid_deps LABEL PATTERN CRATE...: no dependency whose name matches PATTERN.
    forbid_deps() {
        label=$1
        pattern=$2
        shift 2
        for crate in "$@"; do
            [ -f "$metal/$crate/Cargo.toml" ] || continue
            hits=$(dep_names "$metal/$crate/Cargo.toml" | grep -E "$pattern" | sort -u)
            [ -z "$hits" ] || report "$label ($crate)" "$hits"
        done
    }

    echo "metal dir: $metal"
    for crate in proto front back bridge probe conformance bench; do
        if [ -d "$metal/$crate" ]; then
            echo "  crate $crate: present"
        else
            echo "  crate $crate: absent, skipped"
        fi
    done

    # --- proto -----------------------------------------------------------
    forbid_deps "rule 4: proto depends on something (core and alloc only)" '.' proto
    forbid "rule 3a: proto names D3D12/DXGI/HRESULT/Metal/objc2/IR tokens" "$D3D12|$METAL|HRESULT" proto

    # --- front -----------------------------------------------------------
    forbid_deps "rule 3c: front depends on objc2" "$DEP_OBJC2" front
    forbid_deps "rule 2.5: front depends on back" "$DEP_BACK" front
    forbid "rule 2.5: front names the back crate" 'victory[-_]metal[-_]back' front
    forbid "rule 3c: front names Metal/objc2/IR tokens" "$METAL" front

    # --- back ------------------------------------------------------------
    forbid_deps "rule 2.5-2: back depends on a windows crate" "$DEP_WINDOWS" back
    forbid_deps "rule 2.5: back depends on bridge or front" "$DEP_BACK_FORBIDDEN" back proto
    forbid "rule 3b: back names D3D12/DXGI/HRESULT tokens" "$D3D12|HRESULT" back
    # Rule 2.5-6, Metal 4 only: src/ except the M0 exemption described above.
    if [ -d "$metal/back/src" ]; then
        list=$(find "$metal/back/src" -name '*.rs' -type f ! -path "$metal/back/src/m0.rs" -print)
        if [ -n "$list" ]; then
            hits=$(echo "$list" | tr '\n' '\0' | xargs -0 grep -nHE "$METAL3" 2>/dev/null)
            [ -z "$hits" ] || report "rule 2.5-6/11: back uses a Metal 3 command object (only src/m0.rs is exempt)" "$hits"
        fi
    fi

    # --- everyone else ---------------------------------------------------
    forbid "rule 3b: D3D12/DXGI types outside front" "$D3D12" bridge probe conformance bench
    forbid "rule 3c: Metal/objc2/IR tokens outside back" "$METAL" bridge probe conformance bench
    forbid "rule 5: Wine symbols outside bridge" "$WINE" proto front back
    forbid_deps "rule 2.5-2: windows crate in proto" "$DEP_WINDOWS" proto

    # Upstream must not know about us.
    for path in $upstream; do
        [ -e "$path" ] || continue
        # A comment may name the layering rule (mac S6's host_exec.rs does); only code and manifests count.
        hits=$(grep -rnE 'victory-metal|victory_metal' --include='*.rs' --include='Cargo.toml' "$path" 2>/dev/null | grep -vE '^[^:]+\.rs:[0-9]+:[[:space:]]*//' || true)
        [ -z "$hits" ] || report "rule 1: upstream references victory-metal ($path)" "$hits"
    done

    if [ "$failed" = 0 ]; then
        echo "OK    layer rules hold"
    fi
    return $failed
}

self_test() {
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-layers.XXXXXX")
    trap 'rm -rf "$tmp"' EXIT
    status=0

    run_check() { METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up" check >/dev/null 2>&1 && echo pass || echo fail; }
    verdict() { # NAME WANT GOT
        if [ "$3" = "$2" ]; then
            echo "self-test ok   $1 (expected $2)"
        else
            echo "self-test BAD  $1: expected $2, got $3"
            status=1
        fi
    }

    # expect_at NAME RELPATH CONTENT WANT: put CONTENT in metal/RELPATH, run
    # the check on that alone, and require WANT (pass | fail).
    expect_at() {
        rm -rf "$tmp/metal" "$tmp/up"
        mkdir -p "$tmp/up" "$(dirname "$tmp/metal/$2")"
        printf '%s\n' "$3" > "$tmp/metal/$2"
        verdict "$1" "$4" "$(run_check)"
    }
    # expect NAME CRATE CONTENT WANT: as a Rust source of CRATE.
    expect() { expect_at "$1" "$2/src/x.rs" "$3" "$4"; }
    # manifest NAME CRATE CONTENT WANT: as the manifest of CRATE.
    manifest() { expect_at "$1" "$2/Cargo.toml" "$3" "$4"; }

    # --- proto: tokens ---
    expect "clean proto"                        proto  'pub const VERSION: u32 = 1;'                 pass
    expect "ID3D12 in proto"                    proto  'struct X(ID3D12Device);'                     fail
    expect "D3D12_ enum in proto"               proto  'const A: u32 = D3D12_HEAP_TYPE_UPLOAD;'      fail
    expect "IDXGI in proto"                     proto  'use IDXGIFactory;'                           fail
    expect "DXGI_ constant in proto"            proto  'const F: u32 = DXGI_FORMAT_R32_FLOAT;'       fail
    expect "HRESULT in proto"                   proto  'type R = HRESULT;'                           fail
    expect "MTL in proto"                       proto  'type T = MTLBuffer;'                         fail
    expect "MTL4 in proto"                      proto  'type T = MTL4CommandQueue;'                  fail
    expect "objc2 in proto"                     proto  'use objc2::rc::Retained;'                    fail
    expect "IR token in proto"                  proto  'let r = IRRootSignature::new();'            fail
    expect "CAMetal in proto"                   proto  '// a CAMetalLayer'                           fail
    expect "words that merely contain IR pass"  proto  'const FIRST_ELEMENT: u32 = 0; // FIRE, MIRROR' pass
    expect "format ids named without DXGI_ pass" proto 'pub const R32_FLOAT: u32 = 41;'              pass

    # --- proto: dependencies ---
    manifest "proto without dependencies"       proto  $'[package]\nname = "p"\n\n[dependencies]\n# nothing\n' pass
    manifest "proto with a dependency"          proto  $'[dependencies]\nfoo = "1"\n'                 fail
    manifest "proto with a table dependency"    proto  $'[dependencies.foo]\nversion = "1"\n'         fail
    manifest "proto with a dev-dependency"      proto  $'[dev-dependencies]\nfoo = "1"\n'             fail
    manifest "proto with a build-dependency"    proto  $'[build-dependencies]\nfoo = { path = "x" }\n' fail
    manifest "proto with a target dependency"   proto  $'[target.\'cfg(windows)\'.dependencies]\nfoo.workspace = true\n' fail
    manifest "proto with a windows crate"       proto  $'[dependencies]\nwindows-sys = "0.61"\n'     fail
    manifest "proto with a renamed dependency"  proto  $'[dependencies]\nw = { package = "windows", version = "0.62" }\n' fail

    # --- front ---
    expect "D3D12 type in front is allowed"     front  'struct X(ID3D12Device);'                     pass
    expect "Metal type in front"                front  'let d: MTLDevice;'                           fail
    expect "objc2 in front"                     front  'use objc2::rc::Retained;'                    fail
    expect "IR token in front"                  front  'let r = IRRootSignature::new();'             fail
    expect "back crate named in front"          front  'use victory_metal_back::Back;'               fail
    manifest "windows crate in front is allowed" front $'[dependencies]\nwindows = "0.62"\n'         pass
    manifest "objc2 in front manifest"          front  $'[dependencies]\nobjc2 = "0.6"\n'            fail
    manifest "objc2-metal in front manifest"    front  $'[target.\'cfg(target_vendor = "apple")\'.dependencies]\nobjc2-metal = "0.3"\n' fail
    manifest "objc2 table in front manifest"    front  $'[dependencies.objc2-foundation]\nversion = "0.3"\n' fail
    manifest "back in front manifest"           front  $'[dependencies]\nvictory-metal-back = { path = "../back" }\n' fail
    manifest "back renamed in front manifest"   front  $'[dependencies]\nb = { path = "../back", package = "victory-metal-back" }\n' fail
    manifest "proto in front manifest is allowed" front $'[dependencies]\nvictory-metal-proto = { path = "../proto" }\n' pass

    # --- back ---
    expect "Metal type in back is allowed"      back   'use objc2_metal::MTL4CommandQueue;'          pass
    expect "D3D12 type in back"                 back   'struct X(ID3D12Resource);'                   fail
    expect "D3D12_ in back"                     back   'const A: u32 = D3D12_HEAP_TYPE_UPLOAD;'      fail
    expect "IDXGI in back"                      back   'use IDXGIFactory;'                           fail
    expect "HRESULT in back"                    back   'type R = HRESULT;'                           fail
    expect "wine symbol in back"                back   'extern { fn __wine_unix_call(); }'           fail
    manifest "windows crate in back manifest"   back   $'[dependencies]\nwindows = "0.62"\n'         fail
    manifest "windows-sys in back manifest"     back   $'[target.\'cfg(windows)\'.dependencies]\nwindows-sys = "0.61"\n' fail
    manifest "windows table in back manifest"   back   $'[dependencies.windows]\nversion = "0.62"\n' fail
    manifest "objc2 in back manifest is allowed" back  $'[dependencies]\nobjc2 = "0.6"\nobjc2-metal = "0.3"\n' pass
    manifest "front in back manifest"           back   $'[dependencies]\nvictory-metal-front = { path = "../front" }\n' fail
    manifest "bridge in back manifest"          back   $'[dependencies]\nvictory-metal-bridge = { path = "../bridge" }\n' fail
    # rule 2.5-6: every Metal 3 command object, in a file that is not m0.rs
    for tok in 'let q: Retained<ProtocolObject<dyn MTLCommandQueue>>;' \
               'let b: Retained<ProtocolObject<dyn MTLCommandBuffer>>;' \
               'use objc2_metal::{MTLCommandBuffer, MTLDevice};' \
               'dyn MTLBlitCommandEncoder' 'dyn MTLComputeCommandEncoder' 'dyn MTLRenderCommandEncoder' \
               'dyn MTLParallelRenderCommandEncoder' 'enc.useResource_usage(r, u);' 'enc.useHeap(h);' \
               'cb.addCompletedHandler(h);' 'enc.setBytes_length_atIndex(p, 4, 0);'; do
        expect "Metal 3 object in back: $tok"   back   "$tok"                                        fail
    done
    expect "Metal 4 objects in back"            back   'let q: dyn MTL4CommandQueue; let b: dyn MTL4CommandBuffer; let e: dyn MTL4ComputeCommandEncoder; q.signalEvent_value(e, 1);' pass
    expect "MTLCommandBufferStatus is not a command object" back 'let s = MTLCommandBufferStatus::Completed;' pass
    expect_at "Metal 3 object in back/src/m0.rs is exempt" back/src/m0.rs 'let q: dyn MTLCommandQueue; enc.setBytes_length_atIndex(p, 4, 0);' pass
    expect_at "D3D12 in back/src/m0.rs is not exempt" back/src/m0.rs 'struct X(ID3D12Resource);' fail
    expect_at "Wine symbol in back/src/m0.rs is not exempt" back/src/m0.rs 'fn __wine_unix_call();' fail
    expect_at "Metal 3 object in back/src/sub/m0.rs"  back/src/sub/m0.rs 'let q: dyn MTLCommandQueue;' fail
    expect_at "Metal 3 object in back/src/other.rs"   back/src/other.rs 'let q: dyn MTLCommandQueue;' fail
    expect_at "Metal 3 object in back/tests is out of scope (rule covers src/)" back/tests/t.rs 'let q: dyn MTLCommandQueue;' pass

    # --- bridge and the rest ---
    expect "D3D12 type in bridge"               bridge 'let _: D3D12_RESOURCE_DESC;'                 fail
    expect "Metal type in bridge"               bridge 'fn f(_: MTL4CommandQueue) {}'                fail
    expect "layer type in probe"                probe  'let l: CAMetalLayer;'                        fail
    expect "winemac in front"                   front  '// asks winemac for the layer'               fail
    expect "wine symbol in proto"               proto  '// __wine_unix_call'                         fail
    expect "wine symbol in bridge is allowed"   bridge 'static __wine_unix_call_funcs: u32 = 0;'     pass
    expect "words like MTLS-free text pass"     probe  '// residency set, Metal 4 family'            pass
    expect "absent crates are skipped"          conformance '// nothing'                             pass

    # --- upstream ---
    rm -rf "$tmp/metal" "$tmp/up"
    mkdir -p "$tmp/metal" "$tmp/up/crates/x"
    printf '[dependencies]\nvictory-metal-back = { path = "../../metal/back" }\n' > "$tmp/up/crates/x/Cargo.toml"
    got=$(METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up/crates" check >/dev/null 2>&1 && echo pass || echo fail)
    verdict "upstream dependency on victory-metal" fail "$got"

    # --- the real manifests parse (guards the awk) ---
    rm -rf "$tmp/metal" "$tmp/up"
    mkdir -p "$tmp/up" "$tmp/metal/probe"
    cp "$ROOT/metal/bridge/Cargo.toml" "$tmp/real-bridge.toml" 2>/dev/null
    if [ -f "$tmp/real-bridge.toml" ]; then
        names=$(dep_names "$tmp/real-bridge.toml" | sort | tr '\n' ' ')
        case "$names" in
            *windows-sys*) echo "self-test ok   dep_names reads the real bridge manifest ($names)" ;;
            *) echo "self-test BAD  dep_names missed windows-sys in the real bridge manifest: $names"; status=1 ;;
        esac
    fi

    return $status
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi
check
