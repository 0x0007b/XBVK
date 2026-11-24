//! Bridge to `xcl-core`'s RDNA2 disassembler and HLSL → DXIL recompiler.
//!
//! VIctory's native `victory-shader` emits direct DXIL bitcode using VIctory's
//! direct-root-parameter ABI (`b0` user data + root CBVs/SRVs + SM 6.6 heaps),
//! whereas `xcl-core`'s `gpu::recomp` emits HLSL compiled with `dxcompiler.dll`
//! using XCL's UMA `io`/`res`/`mem` ABI (`u0`, `u1`, `u2..`). The two ABIs are
//! kept strictly separate here so a pipeline never mixes incompatible root
//! signatures.

pub use xcl_core::gpu::recomp::pso::{Compiled, Stage};

/// Whether `container` is an AMD FidelityFX (`ACCC`/`FSPX`) compute shader container.
pub fn is_fspx(container: &[u8]) -> bool {
    xcl_core::gpu::recomp::pso::fspx(container).is_some()
}

/// Disassembles raw RDNA2 machine code using `xcl-core`'s AMD ISA decoder.
pub fn disasm_rdna2(code: &[u8]) -> String {
    xcl_core::gpu::rdna2::disassemble(code, 0, false)
        .into_iter()
        .map(|l| format!("{:08x}: {}", l.pc, l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Translates and compiles a console shader container to DXIL using `xcl-core`'s
/// RDNA2 → HLSL → DXC backend (XCL UMA ABI).
pub fn translate_hlsl_dxil(
    container: &[u8],
    stage: Stage,
    split_wave64: bool,
) -> Result<Compiled, String> {
    xcl_core::gpu::recomp::pso::console_to_dxil_for(container, stage, 0, split_wave64)
}

/// Classification of a console shader container via `xcl-core` for diagnostics
/// and fallback reporting when `victory-shader` direct DXIL cannot handle it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackProbe {
    pub is_fspx: bool,
    pub xbox_programs: usize,
    pub stages: Vec<String>,
    pub parse_error: Option<String>,
}

pub fn probe(container: &[u8]) -> FallbackProbe {
    if is_fspx(container) {
        return FallbackProbe {
            is_fspx: true,
            xbox_programs: 1,
            stages: vec!["FSPX(CS)".to_owned()],
            parse_error: None,
        };
    }
    match xcl_core::gpu::xbox::parse(container) {
        Some(parsed) => {
            let xbox_programs = parsed.fssm.iter().map(|f| f.programs.len()).sum();
            let stages = parsed.fssm.iter().map(|f| f.stage_label()).collect();
            FallbackProbe {
                is_fspx: false,
                xbox_programs,
                stages,
                parse_error: None,
            }
        }
        None => FallbackProbe {
            is_fspx: false,
            xbox_programs: 0,
            stages: Vec::new(),
            parse_error: Some("not an Xbox XDAT/XBCH/FSSM shader container".to_owned()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disasm_handles_s_endpgm() {
        // s_endpgm = 0xBF81_0000 (little-endian)
        let text = disasm_rdna2(&0xBF81_0000u32.to_le_bytes());
        assert!(text.to_lowercase().contains("s_endpgm"), "{text}");
    }

    #[test]
    fn is_fspx_rejects_empty_blob() {
        assert!(!is_fspx(&[]));
        assert!(!is_fspx(b"DXBC\0\0\0\0"));
    }
}
