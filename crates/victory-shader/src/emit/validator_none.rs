//! mac S6: the DXIL validator (`dxil.dll`) is a Windows library; on macOS the
//! tests find it unavailable and go without validating (the same path as a
//! Windows machine without it). The Metal Shader Converter's own validation
//! is the check a program meets on the Mac chain.

use std::path::{Path, PathBuf};

pub struct Validator;

impl Validator {
    pub fn default_path() -> Option<PathBuf> {
        None
    }

    pub fn load(_path: Option<&Path>) -> Result<Self, String> {
        Err("the DXIL validator (dxil.dll) is not available on macOS".to_owned())
    }

    pub fn validate(&mut self, _container: &mut [u8]) -> Result<(), String> {
        Ok(())
    }
}
