#!/bin/sh
# Linker for x86_64-pc-windows-gnullvm that links LLVM's libunwind statically
# (llvm-mingw's x86_64-w64-mingw32-clang alone links libunwind.dll, which
# Wine does not have: "Library libunwind.dll not found", status c0000135).
# Set by tools/mac/env.sh; LLVM_MINGW names the llvm-mingw root.
for x in "$@"; do
    case "$x" in
        -lunwind) set -- "$@" "$LLVM_MINGW/x86_64-w64-mingw32/lib/libunwind.a" ;;
        *) set -- "$@" "$x" ;;
    esac
    shift
done
exec "$LLVM_MINGW/bin/x86_64-w64-mingw32-clang" "$@"
