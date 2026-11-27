#!/usr/bin/env python3
"""Mesh shader to compute shader, at the text level of DXC's disassembly.

PROTOTYPE of docs/metal/mesh-compute.md (the lowering the translator would do as
`mac S4`): it stands in for that lowering so the back's runtime can be proven on
real mesh DXIL and on DXC-made synthetic shaders. Not a product tool: it needs
DXC (`tools/metal/dxc-text-tools.sh` builds the disassembler and assembler).

    ms2cs.py MESH.ll OUT.ll OUT.layout [--space N]

`MESH.ll` is `dxcdis` output of a `ms_6_x` DXIL container. The output is the
same program as a `cs_6_x`: every output of the mesh stage becomes a store into
one raw UAV (register u0, space `--space`, default 4000000000) at a fixed
layout, and OUT.layout describes that layout for the back (see
`metal/back/src/mesh_compute.rs`, "The scratch layout"):

    group g, at g * group_stride
        0   u32 vertex count, u32 primitive count (SetMeshOutputCounts)
        16  vertex v at 16 + v * vertex_stride:  the elements, 4 bytes a component
            primitive p at prim_area + p * prim_stride:
                u32 indices (v0 | v1 << 8 | v2 << 16), then the primitive elements

Only what the translator's and DXC's meshes use is handled; anything else is an
error (nothing is guessed): 32-bit components, one row per element or a constant
row, no amplification payload, groups along x only.
"""
import re
import sys

COMP_BYTES = {1: 4, 4: 4, 5: 4, 9: 4}  # bool (stored as i32), i32, u32, f32 only
COMP_NAME = {1: "bool", 4: "int", 5: "uint", 9: "float"}


class Error(Exception):
    pass


def split_ops(s):
    """Top-level comma split of a metadata tuple body."""
    out, depth, cur, in_str = [], 0, "", False
    for ch in s:
        if ch == '"':
            in_str = not in_str
        if not in_str:
            if ch in "{[(":
                depth += 1
            elif ch in "}])":
                depth -= 1
            elif ch == "," and depth == 0:
                out.append(cur.strip())
                cur = ""
                continue
        cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def num(tok):
    m = re.match(r"i(?:8|32|64)\s+(-?\d+)$", tok)
    if not m:
        raise Error(f"not a number: {tok}")
    return int(m.group(1))


class Module:
    def __init__(self, text):
        self.lines = text.split("\n")
        self.md = {}
        self.md_line = {}
        for i, l in enumerate(self.lines):
            m = re.match(r"^!(\d+) = !\{(.*)\}\s*$", l)
            if m:
                self.md[int(m.group(1))] = split_ops(m.group(2))
                self.md_line[int(m.group(1))] = i

    def node(self, tok):
        m = re.match(r"^!(\d+)$", tok)
        if not m:
            raise Error(f"not a node reference: {tok}")
        return int(m.group(1)), self.md[int(m.group(1))]

    def string(self, tok):
        m = re.match(r'^!"(.*)"$', tok)
        return m.group(1) if m else ""


def read_elements(mod, list_tok, what):
    """Elements of a signature list: dicts, in the list's (= id) order."""
    if list_tok == "null":
        return []
    _, items = mod.node(list_tok)
    els = []
    for t in items:
        _, o = mod.node(t)
        ctype, kind = num(o[2]), num(o[3])
        if ctype not in COMP_BYTES:
            raise Error(f"{what}: an element of component type {ctype} (32-bit values only)")
        idx = 0
        if o[4] != "null":
            _, io = mod.node(o[4])
            idx = num(io[0])
        els.append(
            dict(
                id=num(o[0]),
                name=mod.string(o[1]),
                ctype=ctype,
                kind=kind,
                index=idx,
                interp=num(o[5]),
                rows=num(o[6]),
                cols=num(o[7]),
                start_row=num(o[8]),
                start_col=num(o[9]),
            )
        )
    for n, e in enumerate(els):
        if e["id"] != n:
            raise Error(f"{what}: element ids are not 0..n in order")
    return els


def align(n, a):
    return (n + a - 1) // a * a


def convert(text, space):
    mod = Module(text)
    # --- entry point ---------------------------------------------------------
    m = re.search(r"^!dx\.entryPoints = !\{!(\d+)\}", text, re.M)
    if not m:
        raise Error("no !dx.entryPoints")
    entry_id = int(m.group(1))
    entry = mod.md[entry_id]
    sigs_tok, res_tok, props_tok = entry[2], entry[3], entry[4]
    _, sigs = mod.node(sigs_tok)
    verts = read_elements(mod, sigs[1], "vertex outputs")
    prims = read_elements(mod, sigs[2] if len(sigs) > 2 else "null", "primitive outputs")
    _, props = mod.node(props_tok)
    # props: [i32 0, i64 flags, i32 9, !state] (the tag-9 state is the mesh's)
    tags = {}
    for i in range(0, len(props), 2):
        tags[num(props[i])] = props[i + 1]
    if 9 not in tags:
        raise Error("not a mesh shader (no mesh state in the entry properties)")
    _, state = mod.node(tags[9])
    nt_tok = state[0]
    max_v, max_p, topo, payload = (num(state[1]), num(state[2]), num(state[3]), num(state[4]))
    if payload != 0:
        raise Error("an amplification payload (object stage) is not handled")
    if topo != 2:
        raise Error("only triangle topology")
    _, nt = mod.node(nt_tok)
    threads = [num(x) for x in nt]

    # --- layout ---------------------------------------------------------------
    off = 0
    for e in verts:
        e["offset"] = off
        off += 4 * e["rows"] * e["cols"]
    vstride = align(off, 16)
    off = 4
    for e in prims:
        e["offset"] = off
        off += 4 * e["rows"] * e["cols"]
    pstride = align(off, 16)
    vhead = 16
    parea = vhead + max_v * vstride
    gstride = parea + max_p * pstride

    # --- body -------------------------------------------------------------------
    out = []
    in_main = False
    counter = [0]

    def tmp():
        counter[0] += 1
        return f"%mc.{counter[0]}"

    def operand(call, n):
        # The n-th operand of a dx.op call, as text after its type.
        call = call.split("  ;")[0].rstrip()
        body = call[call.index("(") + 1 : call.rindex(")")]
        ops = split_ops(body)
        t = ops[n].strip()
        mm = re.match(r"^(?:i\d+|float|half)\s+(.*)$", t)
        if not mm:
            raise Error(f"operand {n} of {call}")
        return mm.group(1), t.split()[0]

    def store_at(addr, vals, tyname, mask):
        v = list(vals) + ["undef"] * (4 - len(vals))
        ty = "float" if tyname == "f32" else "i32"
        return (
            f"  call void @dx.op.rawBufferStore.{tyname}(i32 140, %dx.types.Handle %mc.h, i32 {addr}, i32 undef, "
            + ", ".join(f"{ty} {x}" for x in v)
            + f", i8 {mask}, i32 4)"
        )

    def element_store(call, els, kind):
        # storeVertexOutput(171, elem, row, col, value, vtx) / storePrimitiveOutput(172, elem, row, col, value, prim)
        elem, _ = operand(call, 1)
        row, _ = operand(call, 2)
        col, _ = operand(call, 3)
        val, _ = operand(call, 4)
        idx, _ = operand(call, 5)
        if not elem.lstrip("-").isdigit():
            raise Error("a store whose element is not a constant")
        e = els[int(elem)]
        fn = re.search(r"dx\.op\.store\w+Output\.(f32|i32)", call)
        if not fn:
            raise Error(f"a store of an unsupported type: {call.strip()}")
        tyname = fn.group(1)
        lines = []
        # byte offset of (row, col) inside the element, 4 bytes a component
        if row.isdigit() and col.isdigit():
            inner = 4 * (int(row) * e["cols"] + int(col))
            inner_expr = str(inner)
        else:
            a = tmp()
            lines.append(f"  {a} = mul i32 {row}, {4 * e['cols']}")
            b = tmp()
            lines.append(f"  {b} = shl i32 {col}, 2")
            c = tmp()
            lines.append(f"  {c} = add i32 {a}, {b}")
            inner_expr = c
        stride, area = (vstride, "%mc.varea") if kind == "v" else (pstride, "%mc.parea")
        m1 = tmp()
        lines.append(f"  {m1} = mul i32 {idx}, {stride}")
        m2 = tmp()
        lines.append(f"  {m2} = add i32 {area}, {m1}")
        m3 = tmp()
        lines.append(f"  {m3} = add i32 {m2}, {e['offset']}")
        m4 = tmp()
        lines.append(f"  {m4} = add i32 {m3}, {inner_expr}")
        lines.append(store_at(m4, [val], tyname, 1))
        return lines

    removed_decl = re.compile(
        r"^declare .*@dx\.op\.(setMeshOutputCounts|emitIndices|storeVertexOutput\.\w+|storePrimitiveOutput\.\w+)\("
    )
    saw_decl = set()
    for line in mod.lines:
        s = line.strip()
        if line.startswith("define void @main()"):
            in_main = True
            out.append(line)
            gx = tmp()
            base = "%mc.base"
            out.append(f"  {gx} = call i32 @dx.op.groupId.i32(i32 94, i32 0)")
            out.append(f"  {base} = mul i32 {gx}, {gstride}")
            out.append(f"  %mc.varea = add i32 {base}, {vhead}")
            out.append(f"  %mc.parea = add i32 {base}, {parea}")
            out.append(
                f"  %mc.h0 = call %dx.types.Handle @dx.op.createHandleFromBinding(i32 217, %dx.types.ResBind {{ i32 0, i32 0, i32 {space}, i8 1 }}, i32 0, i1 false)"
            )
            out.append(
                "  %mc.h = call %dx.types.Handle @dx.op.annotateHandle(i32 216, %dx.types.Handle %mc.h0, %dx.types.ResourceProperties { i32 4107, i32 0 })"
            )
            continue
        if in_main and s == "}":
            in_main = False
        if in_main and "@dx.op." in s and s.startswith("call"):
            if "@dx.op.setMeshOutputCounts" in s:
                v, _ = operand(s, 1)
                p, _ = operand(s, 2)
                out.append(store_at("%mc.base", [v, p], "i32", 3))
                continue
            if "@dx.op.emitIndices" in s:
                prim, _ = operand(s, 1)
                v0, _ = operand(s, 2)
                v1, _ = operand(s, 3)
                v2, _ = operand(s, 4)
                a, b, c, d = tmp(), tmp(), tmp(), tmp()
                out.append(f"  {a} = shl i32 {v1}, 8")
                out.append(f"  {b} = shl i32 {v2}, 16")
                out.append(f"  {c} = or i32 {v0}, {a}")
                out.append(f"  {d} = or i32 {c}, {b}")
                m1, m2 = tmp(), tmp()
                out.append(f"  {m1} = mul i32 {prim}, {pstride}")
                out.append(f"  {m2} = add i32 %mc.parea, {m1}")
                out.append(store_at(m2, [d], "i32", 1))
                continue
            if "@dx.op.storeVertexOutput" in s:
                out.extend(element_store(s, verts, "v"))
                continue
            if "@dx.op.storePrimitiveOutput" in s:
                out.extend(element_store(s, prims, "p"))
                continue
            if "@dx.op.getMeshPayload" in s or "@dx.op.dispatchMesh" in s:
                raise Error("an object-stage payload")
            if "@dx.op.groupId" in s or True:
                pass
        if removed_decl.match(s):
            continue
        out.append(line)
    text2 = "\n".join(out)

    # declarations
    decl = (
        "declare void @dx.op.rawBufferStore.i32(i32, %dx.types.Handle, i32, i32, i32, i32, i32, i32, i8, i32) #1\n"
        "declare void @dx.op.rawBufferStore.f32(i32, %dx.types.Handle, i32, i32, float, float, float, float, i8, i32) #1\n"
    )
    if "declare i32 @dx.op.groupId.i32" not in text2:
        decl += "declare i32 @dx.op.groupId.i32(i32, i32) #0\n"
    if "@dx.op.createHandleFromBinding(i32, %dx.types.ResBind" not in text2:
        decl += "declare %dx.types.Handle @dx.op.createHandleFromBinding(i32, %dx.types.ResBind, i32, i1) #0\n"
    if "declare %dx.types.Handle @dx.op.annotateHandle" not in text2:
        decl += "declare %dx.types.Handle @dx.op.annotateHandle(i32, %dx.types.Handle, %dx.types.ResourceProperties) #0\n"
    text2 = text2.replace("\nattributes #0 =", "\n" + decl + "\nattributes #0 =", 1)
    for ty in (
        "%dx.types.ResBind = type { i32, i32, i32, i8 }",
        "%dx.types.ResourceProperties = type { i32, i32 }",
        "%dx.types.Handle = type { i8* }",
    ):
        if ty not in text2:
            text2 = text2.replace("\ndefine void @main()", f"\n{ty}\n\ndefine void @main()", 1)
    if "%struct.RWByteAddressBuffer = type" not in text2:
        text2 = text2.replace(
            "\ndefine void @main()", "\n%struct.RWByteAddressBuffer = type { i32 }\n\ndefine void @main()", 1
        )

    # metadata
    lines = text2.split("\n")
    top = max(mod.md) + 1
    uav_node = top
    uav_list_node = top + 1
    cs_state_node = top + 2

    def set_md(i, body):
        for k, l in enumerate(lines):
            if l.startswith(f"!{i} = "):
                lines[k] = f"!{i} = !{{{body}}}"
                return
        raise Error(f"no metadata !{i}")

    new_named = []
    if res_tok == "null":
        # No resources at all: a new resource node and its named list.
        res_id = top + 3
        res = ["null", "null", "null", "null"]
        ent_list = list(entry)
        ent_list[3] = f"!{res_id}"
        entry = ent_list
        new_named.append(f"!dx.resources = !{{!{res_id}}}")
        lines.append(f"!{res_id} = !{{null}}")
    else:
        res_id, res = mod.node(res_tok)
        res = list(res)
    while len(res) < 4:
        res.append("null")
    if res[1] == "null":
        uav_id = 0
        res[1] = f"!{uav_list_node}"
        new_list = f"!{uav_node}"
    else:
        lid, lops = mod.node(res[1])
        uav_id = len(lops)
        set_md(lid, ", ".join(list(lops) + [f"!{uav_node}"]))
        new_list = None
    set_md(res_id, ", ".join(res))
    extra = [
        f"!{uav_node} = !{{i32 {uav_id}, %struct.RWByteAddressBuffer* undef, !\"\", i32 {space}, i32 0, i32 1, i32 11, i1 false, i1 false, i1 false, null}}",
    ]
    if new_list is not None:
        extra.append(f"!{uav_list_node} = !{{{new_list}}}")
    # shader model, view id state, entry point, properties
    for k, l in enumerate(lines):
        if l.startswith("!dx.viewIdState"):
            lines[k] = ""
    set_md_sm = None
    sm = re.search(r"^!dx\.shaderModel = !\{!(\d+)\}", "\n".join(lines), re.M)
    sm_id = int(sm.group(1))
    smops = mod.md[sm_id]
    set_md(sm_id, f'!"cs", {smops[1]}, {smops[2]}')
    ent = list(entry)
    ent[2] = "null"
    set_md(entry_id, ", ".join(ent))
    pr = list(props)
    for i in range(0, len(pr), 2):
        if num(pr[i]) == 9:
            pr[i] = "i32 4"
            pr[i + 1] = nt_tok
    set_md(mod.node(props_tok)[0], ", ".join(pr))
    # a dangling viewIdState node is harmless; remove the named list line only
    text3 = "\n".join(lines).rstrip("\n") + "\n" + "\n".join(new_named + extra) + "\n"
    text3 = text3.replace("\n\n\n", "\n\n")
    text3 = text3.replace('target triple = "dxil-ms-dx"', 'target triple = "dxil-ms-dx"')

    # --- layout file ---------------------------------------------------------------
    lay = ["vmcl 1"]
    lay.append(f"threads {threads[0]} {threads[1]} {threads[2]}")
    lay.append(f"maxv {max_v}")
    lay.append(f"maxp {max_p}")
    lay.append(f"vstride {vstride}")
    lay.append(f"pstride {pstride}")
    lay.append(f"vhead {vhead}")
    lay.append(f"parea {parea}")
    lay.append(f"gstride {gstride}")
    lay.append(f"space {space}")
    lay.append("register 0")
    for tag, els in (("v", verts), ("p", prims)):
        for e in els:
            lay.append(
                f"{tag} {e['name']} {e['index']} {e['kind']} {COMP_NAME[e['ctype']]} {e['rows']} {e['cols']} {e['offset']} {e['interp']}"
            )
    return text3, "\n".join(lay) + "\n"


def main():
    args = [a for a in sys.argv[1:]]
    space = 4000000000
    if "--space" in args:
        i = args.index("--space")
        space = int(args[i + 1])
        del args[i : i + 2]
    if len(args) != 3:
        print(__doc__)
        return 2
    src, dst, lay = args
    with open(src, "rb") as f:
        text = f.read().decode("latin-1")
    try:
        t, l = convert(text, space)
    except Error as e:
        print(f"ms2cs: {e}", file=sys.stderr)
        return 1
    with open(dst, "w", newline="\n") as f:
        f.write(t)
    with open(lay, "w") as f:
        f.write(l)
    return 0


if __name__ == "__main__":
    sys.exit(main())
