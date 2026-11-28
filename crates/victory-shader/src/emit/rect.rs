//! The geometry shader that completes the console's rectangle lists (list
//! topology 17, pipeline topology type 19). A rectangle comes as three
//! vertices, three corners of a screen-aligned rectangle; the hardware
//! draws the whole rectangle and continues every vertex shader output
//! linearly into the fourth corner. The host has no such topology: the
//! draw goes out as a triangle list with this shader behind the translated
//! vertex shader, so vertex counts, vertex numbers, indices and indirect
//! arguments stay the title's, and each rectangle arrives here as one
//! triangle.
//!
//! Which vertex is the corner between the other two varies:
//! the title's full-screen rectangles come as top left, bottom left, bottom
//! right (the second), its tiled lighting as top left, bottom left, top
//! right (the first). The corner is the vertex that shares x with one of
//! the others and y with the other; the fourth vertex lies opposite it,
//! `a + b - corner` for the position and every float output. That holds
//! for three vertices with one positive w (then it is the same in clip
//! space and on the screen, whatever the interpolation mode); three that
//! differ in w, or are not such corners, stay the triangle they make.
//!
//! Flat values are the first vertex's: both triangles start with it (the
//! halves on either side of the diagonal through it, turning as the three
//! vertices do), which is the vertex the host takes a flat input from, and
//! the fourth vertex's integer outputs are the first vertex's.
//!
//! Clip distances (mac S4). The geometry emulation of Metal Shader Converter 4.0.3 cannot make its
//! mesh function when the geometry shader *writes* `SV_ClipDistance` (it fails with "Failed to
//! materializeAll"; 29 of the title's 2192 vertex shaders write one), though a clip distance the
//! shader only reads is fine. So the stage keeps the vertex shader's clip distances as inputs and
//! writes none: it does the hardware's clipping itself, once, in the rectangle's own coordinates.
//! The rectangle is the parallelogram `c + u (a - c) + v (b - c)` of its corner `c` and the other
//! two vertices, `(u, v)` in the unit square, and every output, clip distances included, is affine
//! in `(u, v)`: a clip plane is a line in the square. The square is cut by each plane in turn
//! (Sutherland-Hodgman on `4 + planes` slots, a plane adds at most one vertex; slots a cut does
//! not need repeat a vertex, which makes degenerate triangles that draw nothing), and the polygon
//! goes out as one triangle strip, so the pixels drawn are those the hardware draws: where the
//! interpolated distance is not negative (clip space, as D3D12 clips). Attributes of a vertex a
//! cut makes are the same affine function, so they continue as the hardware continues them.
//!
//! shortcut: that the console takes a rectangle's flat values from its
//! first vertex follows from the hardware's provoking-vertex setting for
//! triangles; it is not measured on a rectangle.

use super::bitcode::{Attr, Bin, Term, V, pred};
use super::container::SigElement;
use super::shader::{Geometry, Shader};
use crate::ShaderError;
use crate::dxbc::{Container, FourCC};

/// The semantic a vertex shader's clip distance goes by when it is translated for a rectangle list
/// ([`clip_as_varying`]): an ordinary output the rectangle stage reads and cuts the rectangle by.
pub const CLIP_VARYING: &str = "VICTORYCLIP";

thread_local! {
    static AS_VARYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` (a translation of a vertex shader) so that the vertex shader's `SV_ClipDistance` outputs are
/// written as plain outputs named [`CLIP_VARYING`]. Metal Shader Converter 4.0.3 cannot make the
/// geometry emulation of any pipeline whose vertex stage writes a clip distance, whether or not the
/// geometry shader passes it on; the rectangle stage ([`rect_list_gs`]) does the clipping.
pub fn clip_as_varying<R>(f: impl FnOnce() -> R) -> R {
    let before = AS_VARYING.replace(true);
    let r = f();
    AS_VARYING.set(before);
    r
}

/// Whether a translation in progress on this thread is [`clip_as_varying`].
pub(super) fn clip_as_varying_set() -> bool {
    AS_VARYING.get()
}

/// A vertex output of the rectangle stage that is a clip distance (a real one, or [`CLIP_VARYING`]).
fn is_clip(e: &SigElement) -> bool {
    e.kind == 6 || (e.kind == 0 && e.name == CLIP_VARYING)
}

/// Whether a translated vertex shader (a DXIL container, [`super::HostShader::dxil`]) writes a clip
/// distance: the pipelines this matters for are the rectangle lists with one.
pub fn writes_clip_distance(vs: &[u8]) -> bool {
    Container::parse(vs)
        .ok()
        .and_then(|c| c.part(FourCC::OSG1).map(|p| super::signature(p.data)))
        .and_then(Result::ok)
        .is_some_and(|sig| sig.iter().any(|e| e.kind == 6))
}

/// Vertices emitted per rectangle: two triangles.
const VERTICES: u32 = 6;

/// The clip distances the stage cuts (a vertex shader writes up to eight; the title's write one to four).
const MAX_PLANES: u32 = 4;

/// The scalars a geometry shader may emit per invocation (vertices times
/// output components).
const MAX_SCALARS: u32 = 1024;

/// The geometry shader (a DXIL container) for rectangle lists drawn with
/// the translated vertex shader `vs` and, if the pipeline has one, the
/// translated pixel shader `ps` (both as [`super::HostShader::dxil`]). Its
/// inputs and outputs are the vertex shader's outputs, register for
/// register, so it links with both. A pixel shader that declares a
/// primitive id gets the rectangle's number, which the host leaves to the
/// geometry stage when there is one.
pub fn rect_list_gs(vs: &[u8], ps: Option<&[u8]>) -> Result<Vec<u8>, ShaderError> {
    let sig = |dxil: &[u8], fourcc| -> Result<Vec<SigElement>, ShaderError> {
        let c = Container::parse(dxil)?;
        match c.part(fourcc) {
            Some(part) => super::signature(part.data),
            None => Ok(Vec::new()),
        }
    };
    let outputs = sig(vs, FourCC::OSG1)?;
    let wanted = match ps {
        Some(ps) => sig(ps, FourCC::ISG1)?,
        None => Vec::new(),
    };
    build(outputs, &wanted)
}

/// [`rect_list_gs`] from the vertex shader's outputs and the pixel
/// shader's inputs.
fn build(mut outputs: Vec<SigElement>, ps_inputs: &[SigElement]) -> Result<Vec<u8>, ShaderError> {
    let unsupported = |what| ShaderError::Unsupported { offset: 0, what };
    if outputs.iter().any(|e| e.row.is_none()) {
        return Err(unsupported("a rectangle list output without a register"));
    }
    let Some(position) = outputs
        .iter()
        .position(|e| e.kind == 3 && e.comp == 9 && e.cols == 4)
    else {
        return Err(unsupported("a rectangle list without a four-component position"));
    };
    for e in &mut outputs {
        // Interpolation as `emit_program` declares a vertex shader's
        // outputs; a geometry shader's inputs carry the same modes.
        e.interp = match (e.kind, e.comp) {
            (3, _) => 4,
            (0 | 6 | 7, 9) => 2,
            (0, _) => 1,
            _ => 0,
        };
        e.usage = (1 << e.cols) - 1;
    }
    let mut s = Shader::new(false);
    s.geometry = Some(Geometry {
        max_vertices: VERTICES, // (raised below for clip distances)
    });
    s.inputs = outputs.clone();
    // mac S4: the clip distances stay inputs; the stage writes none (see the head of the file).
    let clip: Vec<usize> = (0..outputs.len()).filter(|&i| is_clip(&outputs[i])).collect();
    let planes: u32 = clip.iter().map(|&i| u32::from(outputs[i].cols)).sum();
    if planes > MAX_PLANES {
        return Err(unsupported("a rectangle list with more clip distances than the stage cuts"));
    }
    // The primitive id the pixel shader declares, as one more output.
    let wanted = ps_inputs
        .iter()
        .find(|e| e.kind == 10 && e.row.is_some())
        .filter(|_| !outputs.iter().any(|o| o.kind == 10));
    let primitive = match wanted {
        Some(e) => {
            // Alone in its columns, and in a row of constant elements.
            let clash = outputs.iter().any(|o| {
                let overlap = o.col < e.col + e.cols && e.col < o.col + o.cols;
                o.row == e.row && (o.interp != 1 || overlap)
            });
            if clash {
                return Err(unsupported(
                    "a rectangle list whose primitive id shares a register with a vertex output",
                ));
            }
            outputs.push(SigElement {
                interp: 1,
                usage: (1 << e.cols) - 1,
                ..e.clone()
            });
            Some(outputs.len() - 1)
        }
        None => None,
    };
    // mac S4: the outputs are the inputs without the clip distances (and the primitive id).
    let slots = if planes > 0 { 4 + planes } else { VERTICES };
    let out_of: Vec<Option<usize>> = {
        let mut next = 0;
        (0..s.inputs.len())
            .map(|i| {
                (!is_clip(&outputs[i])).then(|| {
                    next += 1;
                    next - 1
                })
            })
            .collect()
    };
    outputs = outputs
        .iter()
        .enumerate()
        .filter(|(i, e)| *i >= s.inputs.len() || !is_clip(e))
        .map(|(_, e)| e.clone())
        .collect();
    let primitive = primitive.map(|_| outputs.len() - 1);
    let scalars: u32 = outputs.iter().map(|e| u32::from(e.cols)).sum();
    if scalars * slots > MAX_SCALARS {
        return Err(unsupported(
            "a rectangle list with more output components than a geometry shader emits",
        ));
    }
    s.outputs = outputs;
    let b = s.entry;

    // Every input of the three vertices: [element][column][vertex].
    let mut corners: Vec<Vec<[V; 3]>> = Vec::new();
    for id in 0..s.inputs.len() {
        let mut columns = Vec::new();
        for col in 0..s.inputs[id].cols {
            columns.push([0, 1, 2].map(|vertex| load(&mut s, id, col, vertex)));
        }
        corners.push(columns);
    }

    // One positive w, and no two vertices in one place.
    let (x, y, w) = (
        corners[position][0],
        corners[position][1],
        corners[position][3],
    );
    let zero = s.m.f32(0);
    let mut plain = s.m.cmp(b, pred::OGT, w[0], zero);
    for &other in &w[1..] {
        let one_w = same(&mut s, other, w[0]);
        plain = both(&mut s, plain, one_w);
    }
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        let (same_x, same_y) = (same(&mut s, x[i], x[j]), same(&mut s, y[i], y[j]));
        let together = both(&mut s, same_x, same_y);
        let apart = not(&mut s, together);
        plain = both(&mut s, plain, apart);
    }
    // corner[i]: vertex i is the corner between the other two (the first
    // that shares x with one of them and y with the other).
    let mut corner: Vec<V> = Vec::new();
    for i in 0..3 {
        let (a, c) = ((i + 1) % 3, (i + 2) % 3);
        let (xa, yc) = (same(&mut s, x[i], x[a]), same(&mut s, y[i], y[c]));
        let (xc, ya) = (same(&mut s, x[i], x[c]), same(&mut s, y[i], y[a]));
        let (one, other) = (both(&mut s, xa, yc), both(&mut s, xc, ya));
        let shares = either(&mut s, one, other);
        let mut here = both(&mut s, shares, plain);
        for &found in &corner {
            let earlier = not(&mut s, found);
            here = both(&mut s, here, earlier);
        }
        corner.push(here);
    }

    if planes > 0 {
        s.geometry = Some(Geometry {
            max_vertices: slots,
        });
        let rect = {
            let any = either(&mut s, corner[0], corner[1]);
            either(&mut s, any, corner[2])
        };
        emit_clipped(&mut s, &corners, &clip, &out_of, &corner, rect, primitive, slots);
        return Ok(s.finish());
    }

    // What the six emitted vertices carry: [element][column][vertex].
    let mut emitted: Vec<Vec<[V; 6]>> = Vec::new();
    for (id, columns) in corners.iter().enumerate() {
        let float = s.inputs[id].comp == 9;
        let mut out = Vec::new();
        for &[p0, p1, p2] in columns {
            // The fourth vertex: `a + b - corner`. Integers (flat), and
            // three vertices without a corner, keep the first vertex's
            // value; the second triangle of the latter is then empty.
            let mut fourth = p0;
            if float {
                for (k, (a, c, at)) in [(p1, p2, p0), (p2, p0, p1), (p0, p1, p2)]
                    .into_iter()
                    .enumerate()
                {
                    let sum = s.m.bin(b, Bin::Add, a, c);
                    let opposite = s.m.bin(b, Bin::Sub, sum, at);
                    fourth = s.m.select(b, corner[k], opposite, fourth);
                }
            }
            // Corner first: (v0, v1, v3), (v0, v3, v2). Second: (v0, v1,
            // v2), (v0, v2, v3). Third: (v0, v1, v2), (v0, v3, v1).
            let third = s.m.select(b, corner[0], fourth, p2);
            let fifth = s.m.select(b, corner[1], p2, fourth);
            let sixth = s.m.select(b, corner[0], p2, p1);
            let sixth = s.m.select(b, corner[1], fourth, sixth);
            out.push([p0, p1, third, p0, fifth, sixth]);
        }
        emitted.push(out);
    }

    // The rectangle's number: the triangle's, three vertices a rectangle.
    let mut number = None;
    if let Some(id) = primitive {
        let n = s.op(b, "primitiveID.i32", Attr::ReadNone, s.t.i32, 108, &[]);
        number = Some((id, n));
    }
    let stream = s.m.i8(0);
    for vertex in 0..VERTICES as usize {
        for (id, columns) in emitted.iter().enumerate() {
            let name = if s.outputs[id].comp == 9 {
                "storeOutput.f32"
            } else {
                "storeOutput.i32"
            };
            for (col, values) in columns.iter().enumerate() {
                let args = [
                    s.m.i32(id as u32),
                    s.m.i32(0),
                    s.m.i8(col as u8),
                    values[vertex],
                ];
                s.op(b, name, Attr::NoUnwind, s.t.void, 5, &args);
            }
        }
        if let Some((id, n)) = number {
            let args = [s.m.i32(id as u32), s.m.i32(0), s.m.i8(0), n];
            s.op(b, "storeOutput.i32", Attr::NoUnwind, s.t.void, 5, &args);
        }
        s.op(b, "emitStream", Attr::NoUnwind, s.t.void, 97, &[stream]);
        // The strip ends after the first triangle.
        if vertex == 2 {
            s.op(b, "cutStream", Attr::NoUnwind, s.t.void, 98, &[stream]);
        }
    }
    s.m.terminate(b, Term::Ret);
    Ok(s.finish())
}

/// mac S4: the rectangle cut by its clip distances, as one triangle strip (see the head of the file).
/// `corners[element][column][vertex]` are the stage's inputs, `clip` the elements that are clip
/// distances, `out_of` the output each input is written to (none for a clip distance), `corner` which
/// vertex is the corner (none of them: a triangle), `rect` that one is.
#[allow(clippy::too_many_arguments)]
fn emit_clipped(
    s: &mut Shader,
    corners: &[Vec<[V; 3]>],
    clip: &[usize],
    out_of: &[Option<usize>],
    corner: &[V],
    rect: V,
    primitive: Option<usize>,
    slots: u32,
) {
    let b = s.entry;
    // The corner, the vertex after it and the vertex before it, of a column's three values: `corner[k]` is
    // vertex k; none: the triangle as it comes.
    let roles = |s: &mut Shader, [p0, p1, p2]: [V; 3]| -> (V, V, V) {
        let c = {
            let t = s.m.select(b, corner[2], p2, p0);
            s.m.select(b, corner[1], p1, t)
        };
        let a = {
            let t = s.m.select(b, corner[2], p0, p1);
            s.m.select(b, corner[1], p2, t)
        };
        let o = {
            let t = s.m.select(b, corner[2], p1, p2);
            s.m.select(b, corner[1], p0, t)
        };
        (c, a, o)
    };
    let (zero, one) = (s.m.f32(0), s.m.f32(0x3f80_0000));
    // The unit square, in the order of the vertices around the rectangle (corner, next, opposite, last);
    // a triangle repeats its last vertex.
    let mut poly: Vec<(V, V)> = vec![(zero, zero), (one, zero), (zero, one), (zero, one)];
    poly[2] = (s.m.select(b, rect, one, zero), one);
    // The clip distances, one per plane: (value at the corner, change along u, change along v).
    let mut planes: Vec<(V, V, V)> = Vec::new();
    for &id in clip {
        for col in 0..corners[id].len() {
            let (c, a, o) = roles(s, corners[id][col]);
            let (ga, gb) = (s.m.bin(b, Bin::Sub, a, c), s.m.bin(b, Bin::Sub, o, c));
            planes.push((c, ga, gb));
        }
    }
    for (dc, ga, gb) in planes {
        poly = cut(s, &poly, dc, ga, gb);
    }
    debug_assert_eq!(poly.len() as u32, slots);
    // Every output as `c + u (a - c) + v (b - c)`; integers are the first vertex's.
    let mut affine: Vec<Vec<(V, V, V)>> = Vec::new();
    for (id, columns) in corners.iter().enumerate() {
        let mut cols = Vec::new();
        for &values in columns {
            if s.inputs[id].comp != 9 {
                cols.push((values[0], values[0], values[0]));
                continue;
            }
            let (c, a, o) = roles(s, values);
            cols.push((c, s.m.bin(b, Bin::Sub, a, c), s.m.bin(b, Bin::Sub, o, c)));
        }
        affine.push(cols);
    }
    let number = primitive.map(|id| (id, s.op(b, "primitiveID.i32", Attr::ReadNone, s.t.i32, 108, &[])));
    let stream = s.m.i8(0);
    // The strip: 0, 1, n-1, 2, n-2, ...
    let n = poly.len();
    let (mut lo, mut hi) = (1usize, n - 1);
    let mut order = vec![0usize];
    while lo <= hi {
        order.push(lo);
        lo += 1;
        if lo <= hi {
            order.push(hi);
            hi -= 1;
        }
    }
    for &slot in &order {
        let (u, v) = poly[slot];
        for (id, columns) in affine.iter().enumerate() {
            let Some(out) = out_of[id] else { continue };
            let float = s.inputs[id].comp == 9;
            let name = if float { "storeOutput.f32" } else { "storeOutput.i32" };
            for (col, &(c, du, dv)) in columns.iter().enumerate() {
                let value = if float {
                    let (tu, tv) = (s.m.bin(b, Bin::Mul, u, du), s.m.bin(b, Bin::Mul, v, dv));
                    let sum = s.m.bin(b, Bin::Add, c, tu);
                    s.m.bin(b, Bin::Add, sum, tv)
                } else {
                    c
                };
                let args = [s.m.i32(out as u32), s.m.i32(0), s.m.i8(col as u8), value];
                s.op(b, name, Attr::NoUnwind, s.t.void, 5, &args);
            }
        }
        if let Some((id, n)) = number {
            let args = [s.m.i32(id as u32), s.m.i32(0), s.m.i8(0), n];
            s.op(b, "storeOutput.i32", Attr::NoUnwind, s.t.void, 5, &args);
        }
        s.op(b, "emitStream", Attr::NoUnwind, s.t.void, 97, &[stream]);
    }
    s.op(b, "cutStream", Attr::NoUnwind, s.t.void, 98, &[stream]);
    s.m.terminate(b, Term::Ret);
}

/// The convex polygon `poly` (the vertices in order around it, a vertex repeated where the polygon has
/// fewer than it has slots) cut by the half plane `dc + u ga + v gb >= 0`, with one slot more. The
/// vertices the plane keeps are one run of the cycle: the run, the point where the plane enters before it
/// and the point where it leaves after it; slots past the end repeat the last point. Nothing kept: every slot is the
/// first vertex; everything kept: the polygon.
fn cut(s: &mut Shader, poly: &[(V, V)], dc: V, ga: V, gb: V) -> Vec<(V, V)> {
    let b = s.entry;
    let n = poly.len();
    let zero = s.m.f32(0);
    let one = s.m.f32(0x3f80_0000);
    let dist: Vec<V> = poly
        .iter()
        .map(|&(u, v)| {
            let (tu, tv) = (s.m.bin(b, Bin::Mul, u, ga), s.m.bin(b, Bin::Mul, v, gb));
            let sum = s.m.bin(b, Bin::Add, dc, tu);
            s.m.bin(b, Bin::Add, sum, tv)
        })
        .collect();
    let kept: Vec<V> = dist.iter().map(|&d| s.m.cmp(b, pred::OGE, d, zero)).collect();
    // Where edge i (vertex i to i + 1) meets the plane.
    let mut meet: Vec<(V, V)> = Vec::new();
    // Edge i enters the kept side / leaves it.
    let (mut enters, mut leaves): (Vec<V>, Vec<V>) = (Vec::new(), Vec::new());
    for i in 0..n {
        let j = (i + 1) % n;
        let den = s.m.bin(b, Bin::Sub, dist[i], dist[j]);
        let flat = s.m.cmp(b, pred::OEQ, den, zero);
        let den = s.m.select(b, flat, one, den);
        let t = s.m.bin(b, Bin::SDiv, dist[i], den);
        let point = |s: &mut Shader, from: V, to: V| {
            let d = s.m.bin(b, Bin::Sub, to, from);
            let m = s.m.bin(b, Bin::Mul, t, d);
            s.m.bin(b, Bin::Add, from, m)
        };
        meet.push((point(s, poly[i].0, poly[j].0), point(s, poly[i].1, poly[j].1)));
        let (not_i, not_j) = (not(s, kept[i]), not(s, kept[j]));
        enters.push(both(s, not_i, kept[j]));
        leaves.push(both(s, kept[i], not_j));
    }
    // How many vertices are kept, whether all or none are.
    let (one_i, zero_i) = (s.m.i32(1), s.m.i32(0));
    let mut count = zero_i;
    let mut all = s.m.bool(true);
    let mut any = s.m.bool(false);
    for &k in &kept {
        let one_if = s.m.select(b, k, one_i, zero_i);
        count = s.m.bin(b, Bin::Add, count, one_if);
        all = both(s, all, k);
        any = either(s, any, k);
    }
    // The point the plane enters at, and leaves at.
    let pick = |s: &mut Shader, flags: &[V], from: &dyn Fn(usize) -> (V, V)| -> (V, V) {
        let (mut u, mut v) = from(0);
        for (i, &f) in flags.iter().enumerate() {
            let (cu, cv) = from(i);
            u = s.m.select(b, f, cu, u);
            v = s.m.select(b, f, cv, v);
        }
        (u, v)
    };
    let first = pick(s, &enters, &|i| meet[i]);
    let last = pick(s, &leaves, &|i| meet[i]);
    let mut out = Vec::new();
    for j in 0..=n {
        // Slot j of the cut polygon.
        let run = if j == 0 {
            first
        } else {
            // The vertex j after where the plane enters, while the run lasts.
            let inside = pick(s, &enters, &|i| poly[(i + j) % n]);
            let jj = s.m.i32(j as u32);
            let within = s.m.cmp(b, pred::IULE, jj, count);
            (
                s.m.select(b, within, inside.0, last.0),
                s.m.select(b, within, inside.1, last.1),
            )
        };
        let whole = poly[j.min(n - 1)];
        let nothing = poly[0];
        let u = {
            let t = s.m.select(b, any, run.0, nothing.0);
            s.m.select(b, all, whole.0, t)
        };
        let v = {
            let t = s.m.select(b, any, run.1, nothing.1);
            s.m.select(b, all, whole.1, t)
        };
        out.push((u, v));
    }
    out
}

/// Column `col` of input `id` at vertex `vertex` of the triangle.
fn load(s: &mut Shader, id: usize, col: u8, vertex: u32) -> V {
    let (name, ty) = if s.inputs[id].comp == 9 {
        ("loadInput.f32", s.t.f32)
    } else {
        ("loadInput.i32", s.t.i32)
    };
    let args = [
        s.m.i32(id as u32),
        s.m.i32(0),
        s.m.i8(col),
        s.m.i32(vertex),
    ];
    let entry = s.entry;
    s.op(entry, name, Attr::ReadNone, ty, 4, &args)
}

/// Two floats are equal.
fn same(s: &mut Shader, a: V, b: V) -> V {
    let entry = s.entry;
    s.m.cmp(entry, pred::OEQ, a, b)
}

fn both(s: &mut Shader, a: V, b: V) -> V {
    let entry = s.entry;
    s.m.bin(entry, Bin::And, a, b)
}

fn either(s: &mut Shader, a: V, b: V) -> V {
    let entry = s.entry;
    s.m.bin(entry, Bin::Or, a, b)
}

fn not(s: &mut Shader, a: V) -> V {
    let (entry, on) = (s.entry, s.m.bool(true));
    s.m.bin(entry, Bin::Xor, a, on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(name: &str, index: u32, kind: u8, comp: u8, row: u32, cols: u8) -> SigElement {
        SigElement {
            name: name.into(),
            index,
            kind,
            comp,
            interp: 0,
            row: Some(row),
            col: 0,
            cols,
            usage: 0,
            half: false,
        }
    }

    /// A position, a float interpolant and an integer one.
    fn outputs() -> Vec<SigElement> {
        vec![
            element("SV_Position", 0, 3, 9, 0, 4),
            element("TEXCOORD", 0, 0, 9, 1, 4),
            element("TEXCOORD", 1, 0, 5, 2, 2),
        ]
    }

    /// The stage is a geometry shader of triangles in and six strip
    /// vertices out, with the vertex shader's outputs on both sides and
    /// the primitive id the pixel shader declares; the validator, where
    /// there is one, accepts it.
    #[test]
    fn the_rectangle_stage_is_a_geometry_shader() {
        let wanted = [element("SV_PrimitiveID", 0, 10, 5, 3, 1)];
        let gs = build(outputs(), &wanted).unwrap();
        let c = Container::parse(&gs).unwrap();
        // Program version: geometry (2), shader model 6.6.
        assert_eq!(c.part(FourCC::DXIL).unwrap().data[..4], [0x66, 0, 2, 0]);
        let psv = c.part(FourCC::PSV0).unwrap().data;
        // Triangles in, triangle strips out, stream 0, a position.
        assert_eq!(psv[4..17], [3, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0, 0, 1]);
        // Geometry stage, six vertices, three inputs, four outputs.
        assert_eq!(psv[28..34], [2, 0, 6, 0, 3, 4]);
        let sig = |fourcc| super::super::signature(c.part(fourcc).unwrap().data).unwrap();
        let (ins, outs) = (sig(FourCC::ISG1), sig(FourCC::OSG1));
        assert_eq!(ins.len(), 3);
        assert_eq!(outs.len(), 4);
        for (i, o) in ins.iter().zip(&outs) {
            assert_eq!((&i.name, i.index, i.row), (&o.name, o.index, o.row));
            assert_eq!((i.col, i.cols, i.comp), (o.col, o.cols, o.comp));
        }
        assert_eq!((outs[3].kind, outs[3].row), (10, Some(3)));
        #[cfg(windows)]
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut gs.clone()).unwrap();
            let plain = build(outputs(), &[]).unwrap();
            v.validate(&mut plain.clone()).unwrap();
        }
    }

    /// mac S4: a vertex shader's clip distances (real ones or the plain output the Metal chain's
    /// translation writes them as) are inputs of the stage and no outputs: the stage cuts the rectangle
    /// by them, and as many vertices as the cuts can make are declared (four corners and one per plane).
    #[test]
    fn clip_distances_are_cut_by_the_stage_and_not_passed_on() {
        for kind_name in [(6, "SV_ClipDistance"), (0, super::CLIP_VARYING)] {
            let mut vs = outputs();
            vs.insert(1, element(kind_name.1, 0, kind_name.0, 9, 3, 2));
            let wanted = [element("SV_PrimitiveID", 0, 10, 5, 4, 1)];
            let gs = build(vs, &wanted).unwrap();
            let c = Container::parse(&gs).unwrap();
            let sig = |fourcc| super::super::signature(c.part(fourcc).unwrap().data).unwrap();
            let (ins, outs) = (sig(FourCC::ISG1), sig(FourCC::OSG1));
            assert_eq!(ins.len(), 4);
            assert!(ins.iter().any(|e| e.name == kind_name.1));
            // The position, the two others and the primitive id; no clip distance.
            assert_eq!(outs.len(), 4);
            assert!(outs.iter().all(|e| e.name != kind_name.1 && e.kind != 6));
            // Two planes: four corners and two cuts, six vertices.
            let psv = c.part(FourCC::PSV0).unwrap().data;
            assert_eq!(psv[28..30], [2, 0]);
            assert_eq!(psv[30], 6, "vertices");
        }
        // Four planes of one element: eight vertices; more than four planes are refused.
        let mut vs = outputs();
        vs.insert(1, element("SV_ClipDistance", 0, 6, 9, 3, 4));
        let gs = build(vs.clone(), &[]).unwrap();
        let c = Container::parse(&gs).unwrap();
        assert_eq!(c.part(FourCC::PSV0).unwrap().data[30], 8);
        vs.insert(1, element("SV_ClipDistance", 1, 6, 9, 4, 1));
        assert!(build(vs, &[]).is_err());
    }

    /// Refused with a reason: no position, a primitive id in a register a
    /// vertex output has, more components than six vertices may carry.
    #[test]
    fn rectangle_stages_that_cannot_be_made_are_refused() {
        assert!(build(outputs()[1..].to_vec(), &[]).is_err());
        let taken = [element("SV_PrimitiveID", 0, 10, 5, 1, 1)];
        assert!(build(outputs(), &taken).is_err());
        let mut many = vec![element("SV_Position", 0, 3, 9, 0, 4)];
        many.extend((1..=43).map(|row| element("TEXCOORD", row - 1, 0, 9, row, 4)));
        assert!(build(many, &[]).is_err());
    }

    /// A vertex shader that passes a position and a float4 through, and a
    /// pixel shader that writes the float4.
    #[cfg(windows)]
    fn pass_through() -> (super::super::HostShader, super::super::HostShader) {
        let position = SigElement {
            interp: 4,
            usage: 15,
            ..element("SV_Position", 0, 3, 9, 0, 4)
        };
        let coord = SigElement {
            interp: 2,
            usage: 15,
            ..element("TEXCOORD", 0, 0, 9, 1, 4)
        };
        let mut vs = Shader::new(false);
        vs.inputs = vec![
            SigElement {
                usage: 15,
                ..element("POSITION", 0, 0, 9, 0, 4)
            },
            SigElement {
                interp: 0,
                ..coord.clone()
            },
        ];
        vs.outputs = vec![position.clone(), coord.clone()];
        let mut ps = Shader::new(true);
        ps.inputs = vec![
            SigElement {
                usage: 0,
                ..position
            },
            coord,
        ];
        ps.outputs = vec![SigElement {
            usage: 15,
            ..element("SV_Target", 0, 16, 9, 0, 4)
        }];
        // Vertex: output id from input id. Pixel: target 0 from input 1.
        for (s, copies) in [(&mut vs, &[(0, 0), (1, 1)][..]), (&mut ps, &[(1, 0)][..])] {
            let b = s.entry;
            for &(from, to) in copies {
                for col in 0..4 {
                    let args = [s.m.i32(from), s.m.i32(0), s.m.i8(col), s.m.undef(s.t.i32)];
                    let v = s.op(b, "loadInput.f32", Attr::ReadNone, s.t.f32, 4, &args);
                    let args = [s.m.i32(to), s.m.i32(0), s.m.i8(col), v];
                    s.op(b, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
                }
            }
            s.m.terminate(b, Term::Ret);
        }
        let host = |stage, s: Shader| super::super::HostShader {
            stage,
            dxil: s.finish(),
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            root_indexed_views: Vec::new(),
            vertex_reads: Vec::new(), // mac S1
        };
        (
            host(crate::native::HwStage::Vs, vs),
            host(crate::native::HwStage::Ps, ps),
        )
    }

    /// Three corners of the whole target, the corner between the other two
    /// first (as the tiled lighting sends its tiles), second (as
    /// full-screen rectangles come) or third: with the stage every texel
    /// is drawn and the interpolant continues into the fourth corner's
    /// half; without it part of the target is not drawn.
    #[test]
    #[cfg(windows)]
    fn rectangles_are_drawn_whole() {
        use super::super::host::{Draw, Host};
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let (vs, ps) = pass_through();
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut vs.dxil.clone()).unwrap();
            v.validate(&mut ps.dxil.clone()).unwrap();
        }
        let gs = rect_list_gs(&vs.dxil, Some(&ps.dxil[..])).unwrap();
        let root = host.root_signature(&vs, &ps).unwrap();
        // Position (z 0.5, w 1) and an interpolant that is the position.
        let vertex = |(x, y): (f32, f32)| [x, y, 0.5, 1.0, x, y, 0.0, 1.0];
        for (name, corners) in [
            ("corner first", [(-1.0, 1.0), (-1.0, -1.0), (1.0, 1.0)]),
            ("corner second", [(-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]),
            ("corner third", [(-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]),
        ] {
            let vertices = corners.map(vertex).concat();
            let draw = |gs: Option<&[u8]>| {
                let pso = host
                    .pipeline_with_gs(&root, &vs.dxil, gs, &ps.dxil)
                    .unwrap();
                let d = Draw {
                    roots: Vec::new(),
                    textures: &[],
                    samplers: &[],
                    vertices: &vertices,
                    stride: 32,
                    size: (8, 8),
                    mesh_groups: 0,
                };
                host.draw(&root, &pso, &d).unwrap()
            };
            let whole = draw(Some(&gs[..]));
            for (i, texel) in whole.iter().enumerate() {
                let (x, y) = ((i % 8) as f32, (i / 8) as f32);
                let want = [(x + 0.5) / 4.0 - 1.0, 1.0 - (y + 0.5) / 4.0, 0.0, 1.0];
                assert!(
                    texel.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
                    "{name}: texel {i} is {texel:?}, not {want:?}"
                );
            }
            let half = draw(None);
            let missing = half.iter().zip(&whole).filter(|(a, b)| a != b).count();
            assert!(missing >= 24, "{name}: only {missing} texels need the stage");
        }
    }
}
