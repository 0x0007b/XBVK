//! Translated shaders kept on disk (decision 0003): under the configured
//! cache folder, keyed by the container's content and by the VIctory build
//! that translated it (the BLAKE3 of our executable), so a rebuilt
//! translator never serves an old translation. Entries go through
//! `victory-pipeline`'s content-addressed cache, which writes them
//! atomically; an entry that does not read back is translated again.
//!
//! The entry is our own encoding of [`HostShader`]: a field added to it
//! must be added to `encode` and `decode` too (entries of older builds
//! are ignored anyway).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub use victory_pipeline::Cache;
use victory_pipeline::Converter;
use victory_shader::emit::{HostShader, StaticSampler, UserData};
use victory_shader::resource::RootIndexedView;
use victory_shader::{HwStage, RootType};

static CACHE: OnceLock<Cache> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Serialization schema version for [`encode`] / [`decode`]. Bump whenever
/// [`HostShader`]'s on-disk format changes.
const CACHE_SCHEMA_VERSION: u32 = 1;

/// Opens a shader cache at `dir` (outside the repository).
pub fn open(dir: PathBuf) -> anyhow::Result<Cache> {
    Cache::open(dir)
}

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    match Cache::open(dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// Stores an already-translated [`HostShader`] for `container` into `cache`
/// under the current translator fingerprint.
pub fn store(cache: &Cache, container: &[u8], shader: &HostShader) -> anyhow::Result<bool> {
    store_with_build(cache, build(), container, shader)
}

fn store_with_build(
    cache: &Cache,
    build: &[u8; 32],
    container: &[u8],
    shader: &HostShader,
) -> anyhow::Result<bool> {
    let input = [&build[..], container].concat();
    let encoded = encode(shader);
    let (_, written) = cache.put(&KeySpec, &input, &encoded)?;
    Ok(written)
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// Deterministic translator + settings + schema fingerprint.
fn build() -> &'static [u8; 32] {
    static BUILD: OnceLock<[u8; 32]> = OnceLock::new();
    BUILD.get_or_init(|| {
        let limit = victory_shader::emit::loop_limit();
        if limit != victory_shader::emit::DEFAULT_LOOP_LIMIT {
            tracing::warn!(
                limit,
                "VICTORY_SHADER_LOOP_LIMIT: translated loops stop sooner"
            );
        }
        build_fingerprint(victory_shader::SOURCE_HASH, CACHE_SCHEMA_VERSION, limit)
    })
}

fn build_fingerprint(source_hash: &str, schema_version: u32, loop_limit: u32) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(MAGIC);
    h.update(&schema_version.to_le_bytes());
    h.update(source_hash.as_bytes());
    h.update(&[0]);
    h.update(&loop_limit.to_le_bytes());
    // So is the widest host wave size and the view heap's null slot
    // (they differ by host GPU): a translation cached without them must
    // not be served.
    crate::descriptor::host::probe_view_limit();
    h.update(&victory_shader::emit::host_max_wave_set().to_le_bytes());
    h.update(&victory_shader::emit::view_heap_limit().to_le_bytes());
    // Which group barriers wait is part of the translation too.
    h.update(&[victory_shader::emit::group_sync()]);
    // Wave64 / rounding switches that alter emitted DXIL.
    for name in [
        "VICTORY_WAVE_WIDE",
        "VICTORY_WAVE_DEEP_SKIPS",
        "VICTORY_WAVE_OWN_BITS",
        "VICTORY_LDS_FLAGS",
        "VICTORY_F16_NEAREST",
        "VICTORY_WAVE_LOCAL",
        "VICTORY_STRICT_JOINS",
        "VICTORY_MESH_OUTPUT_STRICT",
    ] {
        h.update(name.as_bytes());
        h.update(b"=");
        if let Some(value) = std::env::var_os(name) {
            h.update(value.as_encoded_bytes());
        }
        h.update(&[0]);
    }
    // And which clean-up passes ran over the translated code.
    h.update(&victory_shader::emit::shader_opt().to_le_bytes());
    *h.finalize().as_bytes()
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    let translate = |c: &[u8]| victory_shader::emit::translate(c).map_err(|e| e.to_string());
    match CACHE.get() {
        Some(cache) => cached(cache, build(), container, translate),
        None => translate(container),
    }
}

/// Converter identity shared by [`Translate`] and [`store_with_build`].
struct KeySpec;

impl Converter for KeySpec {
    fn id(&self) -> &'static str {
        "shader.dxil"
    }
    fn version(&self) -> u32 {
        // The build fingerprint in the input does the versioning.
        1
    }
    fn convert(&self, _input: &[u8]) -> anyhow::Result<Vec<u8>> {
        unreachable!("KeySpec is only used for Cache::put key computation")
    }
}

/// The cache entry's converter: the build and the container in, the
/// encoded translation out.
struct Translate<F> {
    translate: F,
}

impl<F: Fn(&[u8]) -> Result<HostShader, String> + Send + Sync> Converter for Translate<F> {
    fn id(&self) -> &'static str {
        KeySpec.id()
    }
    fn version(&self) -> u32 {
        KeySpec.version()
    }
    fn convert(&self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        Ok(match (self.translate)(&input[32..]) {
            Ok(shader) => encode(&shader),
            Err(err) => encode_err(&err),
        })
    }
}

fn cached(
    cache: &Cache,
    build: &[u8; 32],
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String> + Send + Sync,
) -> Result<HostShader, String> {
    let input = [&build[..], container].concat();
    let converter = Translate { translate };
    for _ in 0..2 {
        let (path, bytes, from_cache) = cache
            .get_or_convert_bytes(&converter, &input)
            .map_err(|e| format!("{e:#}"))?;
        if let Some(entry) = decode_entry(&bytes) {
            let (counter, what) = if from_cache {
                (&HITS, "from the cache")
            } else {
                (&MISSES, "translated and cached")
            };
            counter.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(path = %path.display(), "shader {what}");
            return match entry {
                Entry::Ok(shader) => Ok(shader),
                Entry::Err(err) => Err(err),
            };
        }
        tracing::debug!(path = %path.display(), "unreadable shader cache entry; translating again");
        let _ = std::fs::remove_file(&path);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    (converter.translate)(container)
}

const MAGIC: &[u8; 4] = b"VSH1";
const ERR_MAGIC: &[u8; 4] = b"VSHF";

enum Entry {
    Ok(HostShader),
    Err(String),
}

fn encode_err(err: &str) -> Vec<u8> {
    let bytes = err.as_bytes();
    let mut out = Vec::with_capacity(8 + bytes.len());
    out.extend_from_slice(ERR_MAGIC);
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

fn decode_entry(bytes: &[u8]) -> Option<Entry> {
    if bytes.starts_with(MAGIC) {
        return decode(bytes).map(Entry::Ok);
    }
    if bytes.starts_with(ERR_MAGIC) {
        let len = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        let payload = bytes.get(8..)?;
        if payload.len() != len {
            return None;
        }
        let err = std::str::from_utf8(payload).ok()?.to_owned();
        return Some(Entry::Err(err));
    }
    None
}
const STAGES: [HwStage; 8] = [
    HwStage::Ls,
    HwStage::Hs,
    HwStage::Es,
    HwStage::Gs,
    HwStage::Vs,
    HwStage::Ps,
    HwStage::Ms,
    HwStage::Cs,
];

fn root_code(r: RootType) -> u32 {
    match r {
        RootType::Constants => 0,
        RootType::Descriptor => 1,
        RootType::Table => 2,
        RootType::ImmediateSampler => 3,
        RootType::ResourceHeap => 4,
        RootType::SamplerHeap => 5,
        RootType::Other(x) => 6 + x,
    }
}

fn root_type(code: u32) -> RootType {
    match code {
        0 => RootType::Constants,
        1 => RootType::Descriptor,
        2 => RootType::Table,
        3 => RootType::ImmediateSampler,
        4 => RootType::ResourceHeap,
        5 => RootType::SamplerHeap,
        x => RootType::Other(x - 6),
    }
}

/// Little-endian words and length-prefixed lists.
fn encode(s: &HostShader) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let word = |out: &mut Vec<u8>, v: u32| out.extend(v.to_le_bytes());
    let stage = STAGES.iter().position(|&x| x == s.stage).unwrap_or(0) as u32;
    word(&mut out, stage);
    word(&mut out, s.dxil.len() as u32);
    out.extend(&s.dxil);
    word(&mut out, s.user_data.len() as u32);
    for u in &s.user_data {
        for v in [u.index, u32::from(u.slot), root_code(u.root), u.dword] {
            word(&mut out, v);
        }
    }
    word(&mut out, s.samplers.len() as u32);
    for sampler in &s.samplers {
        for &v in sampler {
            word(&mut out, v);
        }
    }
    word(&mut out, s.static_samplers.len() as u32);
    for t in &s.static_samplers {
        let [u, v, w] = t.address;
        let words = [
            t.filter,
            u,
            v,
            w,
            t.mip_lod_bias.to_bits(),
            t.max_anisotropy,
            t.comparison,
            t.border,
            t.min_lod.to_bits(),
            t.max_lod.to_bits(),
        ];
        for v in words {
            word(&mut out, v);
        }
    }
    for slots in [&s.root_cbvs, &s.root_srvs, &s.root_uavs] {
        word(&mut out, slots.len() as u32);
        out.extend(slots);
    }
    word(&mut out, s.root_indexed_views.len() as u32);
    for view in &s.root_indexed_views {
        for value in [
            u32::from(view.table_slot),
            u32::from(view.constant_slot),
            view.constant_dword,
            view.byte_offset,
            u32::from(view.shift),
        ] {
            word(&mut out, value);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<HostShader> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let (head, rest) = self.0.split_at_checked(n)?;
            self.0 = rest;
            Some(head)
        }
        fn word(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn list(&mut self) -> Option<Vec<u8>> {
            let n = self.word()? as usize;
            Some(self.take(n)?.to_vec())
        }
    }
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return None;
    }
    let stage = *STAGES.get(r.word()? as usize)?;
    let dxil = r.list()?;
    let n = r.word()? as usize;
    let mut user_data = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        user_data.push(UserData {
            index: r.word()?,
            slot: u8::try_from(r.word()?).ok()?,
            root: root_type(r.word()?),
            dword: r.word()?,
        });
    }
    let n = r.word()? as usize;
    let mut samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        samplers.push([r.word()?, r.word()?, r.word()?, r.word()?]);
    }
    let n = r.word()? as usize;
    let mut static_samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        static_samplers.push(StaticSampler {
            filter: r.word()?,
            address: [r.word()?, r.word()?, r.word()?],
            mip_lod_bias: f32::from_bits(r.word()?),
            max_anisotropy: r.word()?,
            comparison: r.word()?,
            border: r.word()?,
            min_lod: f32::from_bits(r.word()?),
            max_lod: f32::from_bits(r.word()?),
        });
    }
    let root_cbvs = r.list()?;
    let root_srvs = r.list()?;
    let root_uavs = r.list()?;
    let n = r.word()? as usize;
    let mut root_indexed_views = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        root_indexed_views.push(RootIndexedView {
            table_slot: u8::try_from(r.word()?).ok()?,
            constant_slot: u8::try_from(r.word()?).ok()?,
            constant_dword: r.word()?,
            byte_offset: r.word()?,
            shift: u8::try_from(r.word()?).ok().filter(|&s| s < 32)?,
        });
    }
    r.0.is_empty().then_some(HostShader {
        stage,
        dxil,
        user_data,
        samplers,
        static_samplers,
        root_cbvs,
        root_srvs,
        root_uavs,
        root_indexed_views,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader() -> HostShader {
        HostShader {
            stage: HwStage::Ps,
            dxil: b"DXBC and the rest".to_vec(),
            user_data: vec![UserData {
                index: 8,
                slot: 1,
                root: RootType::Table,
                dword: 0,
            }],
            samplers: vec![[0x400, 0xc0_0000, 0x18f0_0000, 0]],
            static_samplers: vec![StaticSampler::from_words([
                0x400,
                0xc0_0000,
                0x18f0_0000,
                0,
            ])],
            root_cbvs: vec![2],
            root_srvs: vec![0, 3],
            root_uavs: vec![1],
            root_indexed_views: vec![RootIndexedView {
                table_slot: 1,
                constant_slot: 2,
                constant_dword: 3,
                byte_offset: 32,
                shift: 5,
            }],
        }
    }

    #[test]
    fn translations_come_back_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let build = [7u8; 32];
        let container = b"a console container".as_slice();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the second came from disk");

        // Another build translates again.
        assert_eq!(cached(&cache, &[8; 32], container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 2);

        // A damaged entry is a miss, translated and stored again.
        for e in walk(dir.path()) {
            std::fs::write(e, b"VSH1 broken").unwrap();
        }
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);

        // Failures are cached per build fingerprint so untranslatable shaders
        // do not stall subsequent runs, and re-run when the build changes.
        let fail_runs = AtomicU64::new(0);
        let fail = |_: &[u8]| {
            fail_runs.fetch_add(1, Ordering::Relaxed);
            Err::<HostShader, _>("unsupported".to_owned())
        };
        assert_eq!(cached(&cache, &build, b"other", fail), Err("unsupported".to_owned()));
        assert_eq!(cached(&cache, &build, b"other", fail), Err("unsupported".to_owned()));
        assert_eq!(fail_runs.load(Ordering::Relaxed), 1);
        assert_eq!(cached(&cache, &[9; 32], b"other", fail), Err("unsupported".to_owned()));
        assert_eq!(fail_runs.load(Ordering::Relaxed), 2);
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                files.extend(walk(&e.path()));
            } else {
                files.push(e.path());
            }
        }
        files
    }

    #[test]
    fn entries_round_trip_and_reject_damage() {
        let bytes = encode(&shader());
        assert_eq!(decode(&bytes), Some(shader()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(decode(b"VSH0"), None);

        let err_bytes = encode_err("a select on an untracked mask is not supported");
        assert!(matches!(
            decode_entry(&err_bytes),
            Some(Entry::Err(ref msg)) if msg == "a select on an untracked mask is not supported"
        ));
        assert!(decode_entry(&err_bytes[..err_bytes.len() - 1]).is_none());
        assert!(decode_entry(&[err_bytes.as_slice(), &[0]].concat()).is_none());
    }
}
