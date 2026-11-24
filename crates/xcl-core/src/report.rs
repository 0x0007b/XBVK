//! Minimal JSON report types for XCL GPU/DXC telemetry and tests.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Report {
    #[serde(default)]
    pub shaders: crate::gpu::shader::ShaderStats,
    #[serde(default)]
    pub dxc: crate::gpu::recomp::dxc::Toolchain,
}
