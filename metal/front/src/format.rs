//! DXGI format facts the front needs: bytes per element, block shape, planes.
//! Numbers are `DXGI_FORMAT` values.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatInfo {
    /// Bytes per block (one texel for uncompressed formats).
    pub bytes: u8,
    /// Block size in texels: 4x4 for BCn, 2x1 for packed 4:2:2, 1x1 else.
    pub block_w: u8,
    pub block_h: u8,
    /// 1, or 2 for depth+stencil and the semi-planar video formats.
    pub planes: u8,
}

pub const UNKNOWN: u32 = 0;
pub const R8_TYPELESS: u32 = 60;
pub const R8G8_TYPELESS: u32 = 48;
pub const R16_TYPELESS: u32 = 53;
pub const R16G16_TYPELESS: u32 = 33;
pub const R32_TYPELESS: u32 = 39;
pub const D32_FLOAT_S8X24_UINT: u32 = 20;
pub const D24_UNORM_S8_UINT: u32 = 45;
pub const NV12: u32 = 103;
pub const P010: u32 = 104;
pub const P016: u32 = 105;

const fn f(bytes: u8, block_w: u8, block_h: u8, planes: u8) -> Option<FormatInfo> {
    Some(FormatInfo {
        bytes,
        block_w,
        block_h,
        planes,
    })
}

/// `None` for formats that have no copyable layout here (UNKNOWN, video
/// formats the front does not model, out of range).
pub fn info(format: u32) -> Option<FormatInfo> {
    match format {
        1..=4 => f(16, 1, 1, 1),
        5..=8 => f(12, 1, 1, 1),
        9..=18 => f(8, 1, 1, 1),
        // The R32G8X24 and R24G8 families (typeless, depth-stencil and the two
        // views of each) have two planes, depth and stencil, as D3D12 counts
        // them (a typeless depth texture has both planes: D3DMetal agrees).
        19..=22 => f(8, 1, 1, 2),
        23..=43 => f(4, 1, 1, 1),
        44..=47 => f(4, 1, 1, 2),
        48..=59 => f(2, 1, 1, 1),
        60..=65 => f(1, 1, 1, 1),
        66 => f(1, 8, 1, 1),
        67 => f(4, 1, 1, 1),
        68 | 69 => f(4, 2, 1, 1),
        70..=72 | 79..=81 => f(8, 4, 4, 1),
        73..=78 | 82..=84 | 94..=99 => f(16, 4, 4, 1),
        85 | 86 | 115 => f(2, 1, 1, 1),
        87..=93 => f(4, 1, 1, 1),
        100 | 101 => f(4, 1, 1, 1),
        102 => f(8, 1, 1, 1),
        NV12 => f(1, 1, 1, 2),
        P010 | P016 => f(2, 1, 1, 2),
        107 => f(4, 2, 1, 1),
        108 | 109 => f(8, 2, 1, 1),
        _ => None,
    }
}

/// What one plane of a format looks like: its own format, bytes per block
/// and the shift from the full-size texel grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneInfo {
    pub format: u32,
    pub bytes: u8,
    pub block_w: u8,
    pub block_h: u8,
    /// log2 of the subsampling (0 = full size, 1 = half) in each direction.
    pub shift_x: u8,
    pub shift_y: u8,
}

pub fn plane(format: u32, plane: u32) -> Option<PlaneInfo> {
    let i = info(format)?;
    if plane >= i.planes as u32 {
        return None;
    }
    let p = |format, bytes, sx, sy| {
        Some(PlaneInfo {
            format,
            bytes,
            block_w: 1,
            block_h: 1,
            shift_x: sx,
            shift_y: sy,
        })
    };
    match (format, plane) {
        (19..=22 | 44..=47, 0) => p(R32_TYPELESS, 4, 0, 0),
        (19..=22 | 44..=47, _) => p(R8_TYPELESS, 1, 0, 0),
        (NV12, 0) => p(R8_TYPELESS, 1, 0, 0),
        (NV12, _) => p(R8G8_TYPELESS, 2, 1, 1),
        (P010 | P016, 0) => p(R16_TYPELESS, 2, 0, 0),
        (P010 | P016, _) => p(R16G16_TYPELESS, 4, 1, 1),
        _ => Some(PlaneInfo {
            format,
            bytes: i.bytes,
            block_w: i.block_w,
            block_h: i.block_h,
            shift_x: 0,
            shift_y: 0,
        }),
    }
}

pub fn is_block_compressed(format: u32) -> bool {
    matches!(format, 70..=84 | 94..=99)
}

pub fn is_depth(format: u32) -> bool {
    matches!(format, 20 | 40 | 45 | 55)
}

pub fn is_typeless(format: u32) -> bool {
    matches!(
        format,
        1 | 5
            | 9
            | 15
            | 19
            | 23
            | 27
            | 33
            | 39
            | 44
            | 48
            | 53
            | 60
            | 70
            | 73
            | 76
            | 79
            | 82
            | 90
            | 92
            | 94
            | 97
    )
}
