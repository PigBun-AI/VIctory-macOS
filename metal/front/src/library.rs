//! `ID3D12PipelineLibrary`: names of pipelines made in this process (or
//! saved by an earlier one) with a fingerprint of the description each was
//! made from.
//!
//! The library holds no pipeline and no back object. `StorePipeline` records
//! `name -> fingerprint of the description the pipeline was made from`;
//! `Load*Pipeline(name, desc)` answers by comparing `desc`'s fingerprint with
//! the recorded one and, when they match, makes the pipeline from `desc` the
//! ordinary way (a `SyncOp` that the back answers from its own compiled
//! cache, so a hit costs no compile). A name that is not there, or whose
//! description differs, is `E_INVALIDARG`, the documented "not in the
//! library": the application then makes the pipeline itself and stores it.
//! Because only names and fingerprints are kept, replaying a recorded call
//! sequence gives the same handles, and `Serialize` carries the entries to the
//! next run (no `MTL4Archive`: the compiled code is not in the blob).
//!
//! Pure Rust: the shell object is in `shell/library.rs`.

use std::collections::BTreeMap;

use crate::hresult::Hr;

/// The magic of the blob this front writes.
pub const MAGIC: [u8; 8] = *b"VMPLIB\0\0";
/// The blob's layout version. Version 1 was the empty library (a 16-byte
/// blob with a zero count) and is still read.
pub const VERSION: u32 = 2;
/// Bytes of the header: magic, version, number of entries.
pub const HEADER_SIZE: usize = 16;

/// `D3D12_ERROR_ADAPTER_NOT_FOUND`: the blob is of another adapter (or none
/// of ours).
pub const ERROR_ADAPTER_NOT_FOUND: Hr = Hr(0x887E_0001u32 as i32);
/// `D3D12_ERROR_DRIVER_VERSION_MISMATCH`: a blob of ours from another layout.
pub const ERROR_DRIVER_VERSION_MISMATCH: Hr = Hr(0x887E_0002u32 as i32);

/// A 64-bit FNV-1a hash, the same on every run and build (fingerprints are
/// saved in blobs).
#[derive(Clone, Copy, Debug)]
pub struct Fingerprint(u64);

impl Default for Fingerprint {
    fn default() -> Self {
        Fingerprint(0xcbf2_9ce4_8422_2325)
    }
}

impl Fingerprint {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mixes in `bytes` (and their length, so that parts do not run together).
    pub fn bytes(mut self, bytes: &[u8]) -> Self {
        for b in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    pub fn u64(self, v: u64) -> Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}

/// The names and fingerprints of a library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    entries: BTreeMap<String, u64>,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    /// The library of `CreatePipelineLibrary`'s initial data. An empty blob is
    /// the "no initial data" case. A blob that is not ours is refused as
    /// another adapter's, one of ours that this build cannot read as a version
    /// mismatch (the game layer treats both as "start an empty library").
    pub fn from_blob(blob: &[u8]) -> Result<Library, Hr> {
        if blob.is_empty() {
            return Ok(Library::new());
        }
        if blob.len() < HEADER_SIZE || blob[..8] != MAGIC {
            return Err(ERROR_ADAPTER_NOT_FOUND);
        }
        let word = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap());
        let count = word(12);
        match word(8) {
            1 if count == 0 && blob.len() == HEADER_SIZE => Ok(Library::new()),
            VERSION => {
                let mut lib = Library::new();
                let mut at = HEADER_SIZE;
                for _ in 0..count {
                    let rest = blob.get(at..).ok_or(ERROR_DRIVER_VERSION_MISMATCH)?;
                    if rest.len() < 12 {
                        return Err(ERROR_DRIVER_VERSION_MISMATCH);
                    }
                    let fp = u64::from_le_bytes(rest[..8].try_into().unwrap());
                    let n = u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize;
                    let name = rest
                        .get(12..12 + n)
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .ok_or(ERROR_DRIVER_VERSION_MISMATCH)?;
                    lib.entries.insert(name.to_owned(), fp);
                    at += 12 + n;
                }
                if at != blob.len() {
                    return Err(ERROR_DRIVER_VERSION_MISMATCH);
                }
                Ok(lib)
            }
            _ => Err(ERROR_DRIVER_VERSION_MISMATCH),
        }
    }

    /// `StorePipeline`: `E_INVALIDARG` when the name is taken, as in D3D12.
    pub fn store(&mut self, name: &str, fingerprint: u64) -> Result<(), Hr> {
        if self.entries.contains_key(name) {
            return Err(Hr::E_INVALIDARG);
        }
        self.entries.insert(name.to_owned(), fingerprint);
        Ok(())
    }

    /// Whether `name` is here with a pipeline made from the same description.
    pub fn load(&self, name: &str, fingerprint: u64) -> Result<(), Hr> {
        match self.entries.get(name) {
            Some(&f) if f == fingerprint => Ok(()),
            _ => Err(Hr::E_INVALIDARG),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `GetSerializedSize`.
    pub fn serialized_size(&self) -> usize {
        HEADER_SIZE + self.entries.keys().map(|n| 12 + n.len()).sum::<usize>()
    }

    /// `Serialize` into the caller's buffer: `E_INVALIDARG` when it is too small.
    pub fn serialize_into(&self, out: &mut [u8]) -> Result<(), Hr> {
        let size = self.serialized_size();
        if out.len() < size {
            return Err(Hr::E_INVALIDARG);
        }
        let mut b = Vec::with_capacity(size);
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (name, fp) in &self.entries {
            b.extend_from_slice(&fp.to_le_bytes());
            b.extend_from_slice(&(name.len() as u32).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
        }
        out[..size].copy_from_slice(&b);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(l: &Library) -> Vec<u8> {
        let mut b = vec![0; l.serialized_size()];
        l.serialize_into(&mut b).unwrap();
        b
    }

    #[test]
    fn names_load_with_the_description_they_were_stored_for() {
        let mut l = Library::new();
        l.store("a", 1).unwrap();
        l.store("b", 2).unwrap();
        assert_eq!(l.store("a", 3), Err(Hr::E_INVALIDARG), "the name is taken");
        assert_eq!(l.load("a", 1), Ok(()));
        assert_eq!(l.load("a", 2), Err(Hr::E_INVALIDARG), "another description");
        assert_eq!(l.load("c", 1), Err(Hr::E_INVALIDARG), "not there");
    }

    #[test]
    fn a_library_round_trips_through_its_blob() {
        let mut l = Library::new();
        l.store("pipeline one", 0xdead_beef_0000_0001).unwrap();
        l.store("ünï", 5).unwrap();
        let b = bytes(&l);
        assert_eq!(Library::from_blob(&b), Ok(l.clone()));
        // A bigger buffer is fine, a smaller one is not.
        let mut big = vec![0xFF; b.len() + 9];
        l.serialize_into(&mut big).unwrap();
        assert_eq!(&big[..b.len()], &b[..]);
        assert_eq!(
            l.serialize_into(&mut vec![0; b.len() - 1]),
            Err(Hr::E_INVALIDARG)
        );
        assert_eq!(Library::from_blob(&[]), Ok(Library::new()));
        assert_eq!(
            Library::from_blob(&bytes(&Library::new())),
            Ok(Library::new())
        );
    }

    #[test]
    fn foreign_and_future_blobs_are_refused_with_the_documented_errors() {
        assert_eq!(Library::from_blob(b"garbage"), Err(ERROR_ADAPTER_NOT_FOUND));
        let mut b = bytes(&Library::new());
        b[8] = 9;
        assert_eq!(Library::from_blob(&b), Err(ERROR_DRIVER_VERSION_MISMATCH));
        // A count that the bytes do not hold, and bytes the count does not name.
        let mut l = Library::new();
        l.store("x", 1).unwrap();
        let mut b = bytes(&l);
        b[12] = 2;
        assert_eq!(Library::from_blob(&b), Err(ERROR_DRIVER_VERSION_MISMATCH));
        let mut b = bytes(&l);
        b.push(0);
        assert_eq!(Library::from_blob(&b), Err(ERROR_DRIVER_VERSION_MISMATCH));
        let b = bytes(&l);
        assert_eq!(
            Library::from_blob(&b[..b.len() - 1]),
            Err(ERROR_DRIVER_VERSION_MISMATCH)
        );
    }

    #[test]
    fn the_first_layout_is_still_read() {
        let mut v1 = [0u8; 16];
        v1[..8].copy_from_slice(&MAGIC);
        v1[8] = 1;
        assert_eq!(Library::from_blob(&v1), Ok(Library::new()));
        v1[12] = 1;
        assert_eq!(Library::from_blob(&v1), Err(ERROR_DRIVER_VERSION_MISMATCH));
    }

    #[test]
    fn fingerprints_are_stable_and_separate_parts() {
        let f = |parts: &[&[u8]]| {
            parts
                .iter()
                .fold(Fingerprint::new(), |f, p| f.bytes(p))
                .finish()
        };
        assert_eq!(f(&[b"ab", b"c"]), f(&[b"ab", b"c"]));
        assert_ne!(f(&[b"ab", b"c"]), f(&[b"a", b"bc"]));
        // Saved in blobs: this value must not change.
        assert_eq!(f(&[b"x"]), 5952180755994350292);
    }
}
