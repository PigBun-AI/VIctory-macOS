//! DXIL to a compute pipeline on the Metal 4 device.

use super::{CompileOptions, Msc, MscError, Reflection, RootSignatureDesc, ShaderStage};
use crate::device::Device;
use crate::pipeline::ComputePipeline;

/// A compute pipeline made from DXIL, with what the host needs to bind for it.
#[derive(Clone)]
pub struct ShaderPipeline {
    pub pipeline: ComputePipeline,
    pub reflection: Reflection,
    /// Size of the converted `metallib`, bytes.
    pub metallib_bytes: usize,
}

impl Device {
    /// Converts a compute shader (a DXIL container) with Metal Shader
    /// Converter and makes its pipeline: DXIL + root signature -> `metallib`
    /// -> [`Device::compute_pipeline_from_metallib`].
    ///
    /// Loads `libmetalirconverter.dylib` on first use ([`Msc::get`]).
    pub fn compute_pipeline_from_dxil(
        &self,
        dxil: &[u8],
        root: Option<&RootSignatureDesc>,
        options: &CompileOptions,
    ) -> Result<ShaderPipeline, MscError> {
        let converted = Msc::get()?.compile(dxil, root, options)?;
        if converted.reflection.stage != ShaderStage::Compute {
            return Err(MscError::BadInput(format!(
                "not a compute shader: {:?}",
                converted.reflection.stage
            )));
        }
        let pipeline = self
            .compute_pipeline_from_metallib(&converted.metallib, &converted.reflection.entry)
            .map_err(MscError::Metal)?;
        Ok(ShaderPipeline {
            pipeline,
            metallib_bytes: converted.metallib.len(),
            reflection: converted.reflection,
        })
    }
}
