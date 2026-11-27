#!/bin/sh
# Machine check of layering rule 3 (docs/metal, "分层规矩" in the architecture
# note) and the dependency rules next to it. Exit status 0 = clean.
#
#   rule 3a  proto: no ID3D12 / D3D12_ / IDXGI / MTL / IR tokens
#   rule 3b  D3D12 and DXGI types appear in front only
#   rule 3c  Metal, objc2 and Metal Shader Converter (IR...) tokens appear in back only
#   rule 5   Wine symbols (__wine_*, winemac, macdrv) appear in bridge only
#   deps     back and proto do not depend on windows crates; only back depends on
#            objc2*; upstream crates do not depend on victory-metal-*
#
# Crates that do not exist yet (proto, front, conformance, bench) are reported
# as absent and skipped; the rules apply to them from the day they appear.
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

check() {
    metal=${METAL_DIR:-$ROOT/metal}
    upstream=${UPSTREAM_DIRS:-"$ROOT/crates $ROOT/apps $ROOT/xtask $ROOT/Cargo.toml"}
    failed=0

    # files NAME... : every Rust source and manifest of one crate directory.
    files() {
        [ -d "$metal/$1" ] || return 0
        find "$metal/$1" \( -name target -o -name .git \) -prune -o \
            \( -name '*.rs' -o -name Cargo.toml \) -type f -print
    }

    # forbid LABEL PATTERN CRATE...: no match of PATTERN in the listed crates.
    forbid() {
        label=$1
        pattern=$2
        shift 2
        for crate in "$@"; do
            list=$(files "$crate")
            [ -n "$list" ] || continue
            # shellcheck disable=SC2086
            hits=$(echo "$list" | xargs grep -nE "$pattern" 2>/dev/null)
            if [ -n "$hits" ]; then
                echo "FAIL  $label ($crate):"
                echo "$hits" | sed 's/^/        /'
                failed=1
            fi
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

    forbid "rule 3a: proto names D3D12/DXGI/Metal/IR tokens" "$D3D12|$METAL" proto
    forbid "rule 3b: D3D12/DXGI types outside front" "$D3D12" proto back bridge probe conformance bench
    forbid "rule 3c: Metal/objc2/IR tokens outside back" "$METAL" proto front bridge probe conformance bench
    forbid "rule 5: Wine symbols outside bridge" "$WINE" proto front back
    forbid "deps: back/proto use HRESULT or windows crates" \
        'HRESULT|^[[:space:]]*windows(-[a-z]+)?[[:space:]]*=|^[[:space:]]*windows(-[a-z]+)?\.' proto back
    forbid "deps: front depends on back" 'victory-metal-back' front
    forbid "deps: back depends on bridge or front" 'victory-metal-(bridge|front)' back proto

    # Upstream must not know about us.
    for path in $upstream; do
        [ -e "$path" ] || continue
        hits=$(grep -rnE 'victory-metal|victory_metal' --include='*.rs' --include='Cargo.toml' "$path" 2>/dev/null)
        if [ -n "$hits" ]; then
            echo "FAIL  deps: upstream references victory-metal ($path):"
            echo "$hits" | sed 's/^/        /'
            failed=1
        fi
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

    # expect NAME CRATE CONTENT WANT: put CONTENT in CRATE/src/x.rs, run the
    # check, and require WANT (pass | fail).
    expect() {
        name=$1 crate=$2 content=$3 want=$4
        rm -rf "$tmp/metal" "$tmp/up"
        mkdir -p "$tmp/metal/$crate/src" "$tmp/up"
        printf '%s\n' "$content" > "$tmp/metal/$crate/src/x.rs"
        if METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up" check >/dev/null 2>&1; then
            got=pass
        else
            got=fail
        fi
        if [ "$got" = "$want" ]; then
            echo "self-test ok   $name (expected $want)"
        else
            echo "self-test BAD  $name: expected $want, got $got"
            status=1
        fi
    }

    expect "clean proto"                        proto  'pub const VERSION: u32 = 1;'                 pass
    expect "ID3D12 in proto"                    proto  'struct X(ID3D12Device);'                     fail
    expect "D3D12_ enum in proto"               proto  'const A: u32 = D3D12_HEAP_TYPE_UPLOAD;'      fail
    expect "IDXGI in proto"                     proto  'use IDXGIFactory;'                           fail
    expect "MTL in proto"                       proto  'type T = MTLBuffer;'                         fail
    expect "IR token in proto"                  proto  'let r = IRRootSignature::new();'            fail
    expect "D3D12 type in front is allowed"     front  'struct X(ID3D12Device);'                     pass
    expect "D3D12 type in back"                 back   'struct X(ID3D12Resource);'                  fail
    expect "D3D12 type in bridge"               bridge 'let _: D3D12_RESOURCE_DESC;'                fail
    expect "Metal type in front"                front  'let d: MTLDevice;'                          fail
    expect "objc2 in front"                     front  'use objc2::rc::Retained;'                   fail
    expect "Metal type in back is allowed"      back   'use objc2_metal::MTLDevice;'                pass
    expect "Metal type in bridge"               bridge 'fn f(_: MTL4CommandQueue) {}'               fail
    expect "layer type in probe"                probe  'let l: CAMetalLayer;'                       fail
    expect "wine symbol in back"                back   'extern { fn __wine_unix_call(); }'          fail
    expect "winemac in front"                   front  '// asks winemac for the layer'              fail
    expect "wine symbol in bridge is allowed"   bridge 'static __wine_unix_call_funcs: u32 = 0;'    pass
    expect "HRESULT in back"                    back   'type R = HRESULT;'                          fail
    expect "words like MTLS-free text pass"     probe  '// residency set, Metal 4 family'           pass

    # manifests
    rm -rf "$tmp/metal" "$tmp/up"
    mkdir -p "$tmp/metal/back" "$tmp/metal/front" "$tmp/up"
    printf '[dependencies]\nwindows = "0.62"\n' > "$tmp/metal/back/Cargo.toml"
    if METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up" check >/dev/null 2>&1; then
        echo "self-test BAD  windows crate in back manifest not caught"; status=1
    else
        echo "self-test ok   windows crate in back manifest (expected fail)"
    fi
    printf '[dependencies]\nwindows = "0.62"\n' > "$tmp/metal/front/Cargo.toml"
    printf '[dependencies]\n' > "$tmp/metal/back/Cargo.toml"
    printf '[dependencies]\nobjc2 = "0.6"\n' >> "$tmp/metal/front/Cargo.toml"
    if METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up" check >/dev/null 2>&1; then
        echo "self-test BAD  objc2 in front manifest not caught"; status=1
    else
        echo "self-test ok   objc2 in front manifest (expected fail)"
    fi

    # upstream
    rm -rf "$tmp/metal" "$tmp/up"
    mkdir -p "$tmp/metal" "$tmp/up/crates/x"
    printf '[dependencies]\nvictory-metal-back = { path = "../../metal/back" }\n' > "$tmp/up/crates/x/Cargo.toml"
    if METAL_DIR="$tmp/metal" UPSTREAM_DIRS="$tmp/up/crates" check >/dev/null 2>&1; then
        echo "self-test BAD  upstream dependency on victory-metal not caught"; status=1
    else
        echo "self-test ok   upstream dependency on victory-metal (expected fail)"
    fi

    return $status
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi
check
