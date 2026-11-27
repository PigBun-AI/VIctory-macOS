//! Mac patch: the offline translator commands (`translate`, `translate
//! --warm`, `shaders`) need the Windows host (victory-shader's `host`
//! feature drives a D3D12 device and the DXIL validator), so on other hosts
//! they only say so, as VIctory-macOS did. Windows builds use `translate.rs`.
//! See docs/mac/PATCHES.md, patch M1.

use std::path::{Path, PathBuf};

use anyhow::Result;

const NEEDS_WINDOWS: &str = "translate needs a Windows host for now (on a Mac, run victory.exe under Wine: tools/macos/victory.sh translate ...)";

pub fn warm(_paths: Vec<PathBuf>, _file: &Path, _jobs: Option<usize>) -> Result<()> {
    anyhow::bail!(NEEDS_WINDOWS)
}

pub fn run(
    _paths: Vec<PathBuf>,
    _validate: bool,
    _pipelines: bool,
    _list: bool,
    _jobs: Option<usize>,
) -> Result<()> {
    anyhow::bail!(NEEDS_WINDOWS)
}

pub fn precompile(_paths: Vec<PathBuf>, _jobs: Option<usize>) -> Result<()> {
    anyhow::bail!(NEEDS_WINDOWS)
}
