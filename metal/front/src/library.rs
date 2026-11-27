//! `ID3D12PipelineLibrary`, the minimal one: a library that never holds a
//! pipeline. Stores are accepted and dropped, every load answers
//! `E_INVALIDARG` (the documented "not in the library"), and the library
//! serializes to a small valid blob that creating a library from it accepts.
//! The game layer's fallback (`victory-gpu`, `pipeline/library.rs`) then
//! makes the pipeline and stores it again, as it does for an empty library on
//! any driver. A persistent pipeline cache belongs to the back (MTL4Archive,
//! later) and would replace this.
//!
//! Pure Rust: the shell object is in `shell/library.rs`.

use crate::hresult::Hr;

/// The magic of the blob this front writes.
pub const MAGIC: [u8; 8] = *b"VMPLIB\0\0";
/// The blob's layout version: magic, version, number of stored pipelines (0).
pub const VERSION: u32 = 1;
/// Bytes of the serialized library.
pub const SERIALIZED_SIZE: usize = 16;

/// `D3D12_ERROR_ADAPTER_NOT_FOUND`: the blob is of another adapter (or none
/// of ours).
pub const ERROR_ADAPTER_NOT_FOUND: Hr = Hr(0x887E_0001u32 as i32);
/// `D3D12_ERROR_DRIVER_VERSION_MISMATCH`: a blob of ours from another layout.
pub const ERROR_DRIVER_VERSION_MISMATCH: Hr = Hr(0x887E_0002u32 as i32);

/// The serialized empty library.
pub fn serialize() -> [u8; SERIALIZED_SIZE] {
    let mut b = [0u8; SERIALIZED_SIZE];
    b[..8].copy_from_slice(&MAGIC);
    b[8..12].copy_from_slice(&VERSION.to_le_bytes());
    b
}

/// Checks the initial data of `CreatePipelineLibrary`. An empty blob is the
/// "no initial data" case. A blob that is not ours is refused as another
/// adapter's, one of ours that this build cannot read as a version mismatch
/// (the game layer treats both as "start an empty library").
pub fn check_blob(blob: &[u8]) -> Result<(), Hr> {
    if blob.is_empty() {
        return Ok(());
    }
    if blob.len() < SERIALIZED_SIZE || blob[..8] != MAGIC {
        return Err(ERROR_ADAPTER_NOT_FOUND);
    }
    if blob[8..12] != VERSION.to_le_bytes() || blob[12..16] != [0; 4] {
        return Err(ERROR_DRIVER_VERSION_MISMATCH);
    }
    Ok(())
}

/// `Serialize` into the caller's buffer: `E_INVALIDARG` when it is too small.
pub fn serialize_into(out: &mut [u8]) -> Result<(), Hr> {
    if out.len() < SERIALIZED_SIZE {
        return Err(Hr::E_INVALIDARG);
    }
    out[..SERIALIZED_SIZE].copy_from_slice(&serialize());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_library_round_trips() {
        assert_eq!(check_blob(&serialize()), Ok(()));
        assert_eq!(check_blob(&[]), Ok(()));
    }

    #[test]
    fn foreign_and_future_blobs_are_refused_with_the_documented_errors() {
        assert_eq!(check_blob(b"garbage"), Err(ERROR_ADAPTER_NOT_FOUND));
        let mut b = serialize();
        b[8] = 2;
        assert_eq!(check_blob(&b), Err(ERROR_DRIVER_VERSION_MISMATCH));
        let mut b = serialize();
        b[12] = 1;
        assert_eq!(check_blob(&b), Err(ERROR_DRIVER_VERSION_MISMATCH));
    }

    #[test]
    fn serialize_checks_the_buffer() {
        let mut small = [0u8; SERIALIZED_SIZE - 1];
        assert_eq!(serialize_into(&mut small), Err(Hr::E_INVALIDARG));
        let mut big = [0xFFu8; 40];
        serialize_into(&mut big).unwrap();
        assert_eq!(&big[..SERIALIZED_SIZE], &serialize());
    }
}
