//! Runtime hex byte patching (`XCLVIC_PATCH` / `XCL_PATCH`) ported from Palmwave.
//!
//! Format: `<module>+0x<rva>:<old_hex>=<new_hex>;...`
//! Each patch is applied only if the mapped image holds `old_hex` at `rva`.

use crate::util::dll_key;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Patch {
    pub module: String,
    pub rva: usize,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn parse_patch(entry: &str) -> Option<Patch> {
    let (target, bytes) = entry.split_once(':')?;
    let (module, rva) = target.trim().rsplit_once('+')?;
    let rva = rva.trim();
    let rva = usize::from_str_radix(
        rva.strip_prefix("0x")
            .or_else(|| rva.strip_prefix("0X"))
            .unwrap_or(rva),
        16,
    )
    .ok()?;
    let (old, new) = bytes.split_once('=')?;
    let (old, new) = (parse_hex(old)?, parse_hex(new)?);
    (old.len() == new.len() && !module.trim().is_empty()).then(|| Patch {
        module: dll_key(module),
        rva,
        old,
        new,
    })
}

/// Parses semicolon-separated `<module>+0x<rva>:<old>=<new>` entries.
pub fn parse_patches(spec: &str) -> Vec<Patch> {
    spec.split(';')
        .filter(|e| !e.trim().is_empty())
        .filter_map(|e| {
            let p = parse_patch(e);
            if p.is_none() {
                crate::xlog_warn!(
                    "PATCH: '{}' is not <module>+0x<rva>:<hex bytes>=<as many hex bytes> – left out",
                    e.trim()
                );
            }
            p
        })
        .collect()
}

/// Applies patches matching `module` onto `image` when the existing bytes match `old`.
pub fn apply_patches(spec: &str, module: &str, image: &mut [u8]) -> usize {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let target = dll_key(module);
    let mut applied = 0;
    for p in parse_patches(spec).into_iter().filter(|p| p.module == target) {
        let Some(at) = p
            .rva
            .checked_add(p.old.len())
            .and_then(|end| image.get_mut(p.rva..end))
        else {
            crate::xlog_warn!("PATCH: {module}+{:#x} is outside the image – not written", p.rva);
            continue;
        };
        if *at != *p.old {
            crate::xlog_warn!(
                "PATCH: {module}+{:#x} holds {} instead of {} – not written (another version?)",
                p.rva,
                hex(at),
                hex(&p.old)
            );
            continue;
        }
        at.copy_from_slice(&p.new);
        applied += 1;
        crate::xlog_info!(
            "PATCH: {module}+{:#x}: {} -> {}",
            p.rva,
            hex(&p.old),
            hex(&p.new)
        );
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_applies_matching_patches() {
        let mut img = vec![0x00, 0x73, 0x2e, 0xff, 0x74, 0x04];
        let spec = "game.exe+0x1:732e=9090; other.dll+0x0:00=ff; bad_entry; game.exe+0x4:74 04=90 90";
        let applied = apply_patches(spec, "GAME.EXE", &mut img);
        assert_eq!(applied, 2);
        assert_eq!(img, vec![0x00, 0x90, 0x90, 0xff, 0x90, 0x90]);
    }

    #[test]
    fn skips_mismatched_bytes_or_out_of_bounds() {
        let mut img = vec![0x75, 0x2e];
        let spec = "game.exe+0x0:732e=9090; game.exe+0x10:00=ff";
        assert_eq!(apply_patches(spec, "game.exe", &mut img), 0);
        assert_eq!(img, vec![0x75, 0x2e]);
    }
}
