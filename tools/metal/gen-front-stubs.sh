#!/bin/sh
# Generates metal/front/src/shell/{stubs,gen}.rs; `--check` fails when the
# committed files are not what the generator writes. The generator is Python
# (embedded below, because the repository's asset policy only takes .sh
# scripts, not .py): run from anywhere, it works on this repository.
set -eu
GEN_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export GEN_ROOT
exec python3 - "$@" <<'PY'
"""Generate metal/front/src/shell/stubs.rs.

`#[implement]` needs every method of every interface a COM object claims
(ID3D12Device14 alone is ~90 methods plus its bases). This script reads the
`*_Impl` trait definitions out of the `windows` crate the workspace is locked
to and writes, for every object in OBJECTS, one `impl X_Impl for Obj_Impl` per
interface in its chain. Each method either

  * calls the hand-written `x_<Method>` of the object (found by scanning the
    object's source file for `fn x_<Method>(`), or
  * is a stub: `E_NOTIMPL` for HRESULT-returning methods, a no-op / zeroed
    value otherwise (a call outside the current milestone), or
  * for the four private-data / name methods of Object interfaces, forwards to
    the `private` field every object has.

Run (from the repository root) after changing OBJECTS or hand-written `x_`
methods:  tools/metal/gen-front-stubs.sh
`--check` fails if the committed file is not what the generator writes.
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.environ["GEN_ROOT"]
FRONT = os.path.join(ROOT, "metal", "front", "src", "shell")
OUT = os.path.join(FRONT, "stubs.rs")

# struct name -> (leaf interface, source file with its hand-written x_ methods)
# A struct named in VOID_HOOK reports every unimplemented void method through
# `self.unsupported_call()` (a command list: the error surfaces at Close).
VOID_HOOK = {"ListObj"}
OBJECTS = {
    "FactoryObj": ("IDXGIFactory7", "dxgi.rs"),
    "AdapterObj": ("IDXGIAdapter4", "dxgi.rs"),
    "SwapChainObj": ("IDXGISwapChain4", "swapchain.rs"),
    "DeviceObj": ("ID3D12Device14", "device.rs+ext.rs"),
    "QueueObj": ("ID3D12CommandQueue", "objects.rs+ext.rs"),
    "AllocatorObj": ("ID3D12CommandAllocator", "objects.rs"),
    "ListObj": ("ID3D12GraphicsCommandList6", "list.rs+ext.rs"),
    "FenceObj": ("ID3D12Fence1", "objects.rs"),
    "QueryHeapObj": ("ID3D12QueryHeap", "ext.rs"),
    "ResourceObj": ("ID3D12Resource2", "objects.rs"),
    "HeapObj": ("ID3D12DescriptorHeap", "objects.rs"),
    "MemHeapObj": ("ID3D12Heap", "objects.rs"),
    "RootSigObj": ("ID3D12RootSignature", "objects.rs"),
    "PsoObj": ("ID3D12PipelineState", "objects.rs"),
    "CmdSigObj": ("ID3D12CommandSignature", "objects.rs"),
    "LibraryObj": ("ID3D12PipelineLibrary1", "library.rs"),
    "RootSigDeserializerObj": ("ID3D12RootSignatureDeserializer", "serialize.rs"),
    "VersionedDeserializerObj": ("ID3D12VersionedRootSignatureDeserializer", "serialize.rs"),
}

# Object-interface methods every object answers from its `private` field.
PRIVATE = {
    "ID3D12Object": {"GetPrivateData", "SetPrivateData", "SetPrivateDataInterface", "SetName"},
    "IDXGIObject": {"GetPrivateData", "SetPrivateData", "SetPrivateDataInterface"},
}
# ID3D12DeviceChild::GetDevice: every child has `device_obj`.
CHILD_GET_DEVICE = "ID3D12DeviceChild"


def windows_src():
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--offline", "--manifest-path", os.path.join(ROOT, "metal", "Cargo.toml")]
        )
    )
    for p in meta["packages"]:
        if p["name"] == "windows":
            return os.path.dirname(p["manifest_path"]), p["version"]
    sys.exit("windows crate not in the lock file")


def split_args(s):
    out, depth, cur = [], 0, ""
    for ch in s:
        if ch in "<(":
            depth += 1
        elif ch in ">)":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def fix_path(t):
    t = t.replace("super::super::", "windows::Win32::")
    t = t.replace("super::", "windows::Win32::Graphics::")
    t = t.replace("windows_core::", "windows::core::")
    return t


def parse_traits(base):
    """interface -> (parent trait name or None, [(cfg, name, [(argname, argtype)], ret)])"""
    traits = {}
    for rel in ("Direct3D12/mod.rs", "Dxgi/mod.rs"):
        path = os.path.join(base, "src", "Windows", "Win32", "Graphics", rel)
        lines = open(path, encoding="utf-8").read().split("\n")
        i = 0
        while i < len(lines):
            m = re.match(r"pub trait (\w+)_Impl: ([\w:]+)\s*\{", lines[i])
            if not m:
                i += 1
                continue
            name, parent = m.group(1), m.group(2)
            parent = parent[:-5] if parent.endswith("_Impl") else None
            methods = []
            i += 1
            cfg = []
            while lines[i] != "}":
                l = lines[i].strip()
                if l.startswith("#["):
                    cfg.append(l)
                else:
                    mm = re.match(r"fn (\w+)(?:<[^>]*>)?\(&self(?:, (.*))?\)(?: -> (.*))?;$", l)
                    if mm:
                        args = []
                        for a in split_args(mm.group(2) or ""):
                            an, at = a.split(":", 1)
                            args.append((an.strip(), fix_path(at.strip())))
                        methods.append((cfg, mm.group(1), args, fix_path(mm.group(3)) if mm.group(3) else None))
                        cfg = []
                i += 1
            traits[name] = (parent, methods)
    return traits


def chain(traits, leaf):
    c = []
    while leaf:
        c.append(leaf)
        leaf = traits[leaf][0]
    return list(reversed(c))


def hand_written(file):
    # "a.rs+b.rs": the object's x_ methods live in several files.
    return "\n".join(open(os.path.join(FRONT, f), encoding="utf-8").read() for f in file.split("+"))


def methods_of(src, struct):
    """x_ methods inside `impl <struct>_Impl {` blocks of a source file (the
    `_Impl` type is the one `#[implement]` makes; `self.to_object()` works there)."""
    found = set()
    for m in re.finditer(r"impl\s+%s_Impl\s*\{" % struct, src):
        depth, i = 1, m.end()
        start = i
        while depth:
            c = src[i]
            depth += (c == "{") - (c == "}")
            i += 1
        found |= set(re.findall(r"fn x_(\w+)\(", src[start:i]))
    return found


def emit():
    base, version = windows_src()
    traits = parse_traits(base)
    out = []
    w = out.append
    w("// @generated by tools/metal/gen-front-stubs.sh from the `windows` crate %s." % version)
    w("// Do not edit: change the hand-written `x_<Method>` methods or the script and rerun it.")
    w("#![allow(non_snake_case, clippy::too_many_arguments, clippy::missing_safety_doc, unused_variables)]")
    w("")
    w("use windows::Win32::Foundation::*;")
    w("use windows::Win32::Graphics::Direct3D12::*;")
    w("use windows::Win32::Graphics::Dxgi::*;")
    w("")
    w("use super::device::*;")
    w("use super::dxgi::*;")
    w("use super::ext::*;")
    w("use super::library::*;")
    w("use super::list::*;")
    w("use super::objects::*;")
    w("use super::serialize::*;")
    w("use super::swapchain::*;")
    for struct, (leaf, file) in OBJECTS.items():
        have = methods_of(hand_written(file), struct)
        used = set()
        for iface in chain(traits, leaf):
            w("")
            w("impl %s_Impl for %s_Impl {" % (iface, struct))
            for cfg, name, args, ret in traits[iface][1]:
                for c in cfg:
                    w("    " + c)
                sig = ", ".join(["&self"] + ["%s: %s" % a for a in args])
                call_args = ", ".join(a[0] for a in args)
                w("    fn %s(%s)%s {" % (name, sig, " -> " + ret if ret else ""))
                w('        crate::callstats::call("%s::%s");' % (iface, name))
                if name in PRIVATE.get(iface, ()):
                    ptr_args = call_args
                    w("        self.private.%s_%s(%s)" % ("d3d12" if iface == "ID3D12Object" else "dxgi", name, ptr_args))
                elif iface == CHILD_GET_DEVICE and name == "GetDevice":
                    w("        super::query_device(&self.device_obj, %s)" % call_args)
                elif name in have and (iface, name) not in used:
                    used.add((iface, name))
                    w("        self.x_%s(%s)" % (name, call_args))
                elif ret is None:
                    w('        super::note_notimpl("%s::%s");' % (iface, name))
                    w("        self.unsupported_call();" if struct in VOID_HOOK else "        ")
                elif ret == "windows::core::HRESULT":
                    w('        super::note_notimpl("%s::%s");' % (iface, name))
                    w("        E_NOTIMPL")
                elif ret.startswith("windows::core::Result"):
                    w('        super::note_notimpl("%s::%s");' % (iface, name))
                    w("        Err(windows::core::Error::from(E_NOTIMPL))")
                else:
                    w('        super::note_notimpl("%s::%s");' % (iface, name))
                    w("        unsafe { core::mem::zeroed() }")
                w("    }")
            w("}")
        unused = have - {n for (_, n) in used}
        if unused:
            sys.exit("%s: hand-written x_ methods that no interface asks for: %s" % (struct, sorted(unused)))
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    text = emit()
    if "--check" in sys.argv:
        cur = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        if cur != text:
            sys.exit("%s is stale: run tools/metal/gen-front-stubs.sh" % OUT)
        print("stubs.rs is current")
    else:
        os.makedirs(FRONT, exist_ok=True)
        open(OUT, "w", encoding="utf-8").write(text)
        print("wrote", OUT, len(text.splitlines()), "lines")
PY
