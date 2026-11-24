# XBVK / XCLVIC — Xbox Series X|S Compatibility Layer

**XBVK (XCLVIC)** is an experimental native Windows compatibility layer built in Rust that runs Xbox Series X|S (`Scarlett`) x86-64 titles directly inside a host Windows process.

> **Graphics API architecture:** Translates console **D3D12.X** command streams and **RDNA2** shaders natively onto **Windows Direct3D 12 (Shader Model 6.6 / DXIL)** (not Vulkan).

## Current Status & Active Work

**Status: Pre-alpha (under active development; not playable in-world yet).**

### What works today
- **Loader & PE/TLS runtime (`victory-loader`, `victory-pe`):** Maps title executables and DLLs in-process, rewrites static TLS accesses, supports optional runtime byte patching via `XCLVIC_PATCH`, and writes `module+rva` crash backtraces to `crashes.log`.
- **Console HLE & virtual filesystem (`victory-hle`):** Implements core `kernelx`, `xgameruntime`, `dstorage`, `gameinput`, `audio`, and drive redirection (`G:` read-only with copy-on-write overlay, `T:` temp storage), with live mirrored logging to `%LOCALAPPDATA%\victory\data\debug\latest.log`.
- **Boot & UI flow:** Startup sequence, intro audio/video playback, offline profile/save persistence, and loading stages run up to world entry; diagnostics builds capture PNG frames (`latest.png`, boot capture, periodic captures, and black-frame anomalies) under `%LOCALAPPDATA%\victory\data\debug\frames\`.

### Known Issues & Ongoing Work
Active engineering work is focused on stabilizing world rendering, shader compatibility, and GPU execution stability:

| Area | Current Problem | Active Work / Mitigation |
|---|---|---|
| **First-frame shader stalls & frame drops** | World entry triggers translation of large numbers of shaders and PSOs simultaneously, causing multi-hundred-millisecond frame spikes and stutters | Disk shader caching, parallel pipeline library loading, MRU early-pipeline warmup list, adaptive per-frame compile budgets (`VICTORY_PIPELINE_BUDGET_MS`), shader loop/spin safety caps |
| **Driver TDR timeouts (`DXGI_ERROR_DEVICE_HANG`)** | Heavy command lists, long shader loops, or mismatched barriers can exceed the Windows GPU watchdog timeout (~2 s) | Per-list command submission splitting, configurable command-list cost splitting, loop/spin limits, DRED breadcrumbs, host TDR registry checks |
| **Wave64 shaders on Wave32 GPUs** | Console RDNA2 executes in Wave64, while some PC GPUs default to Wave32; lane ballots, `v_readfirstlane`, and divergent barriers can produce wrong results or hang | Compute Wave64 emulation via group-shared scratch, divergent barrier trimming on narrow-wave hosts, ongoing Mesh stage Wave64 emulation work |
| **Mesh shader host limits** | Console mesh shaders can declare wide outputs exceeding D3D12's 32 KiB output budget | Vertex/primitive output clamping to stay within 32 KiB and `GS_ALLOC_REQ` output count lowering |
| **Texture readbacks & RT/UAV formats** | `CopyTextureRegion` texture-to-buffer readbacks and unsupported host RT/UAV formats (`R9G9B9E5_SHAREDEXP`, `B4G4R4A4`, `B5G5R5A1`) need staging and safe host promotion | Staged readback copies, typeless/format promotion guards, skipping invalid RTV creation when `ALLOW_RENDER_TARGET` is stripped (**currently fixing an intermittent unit test failure in texture readback stabilization**) |
| **Descriptors, root views & indirect execution** | Empty table slots read as wrong resource kinds, unaligned raw buffer views, out-of-range heap indices, or oversized indirect counts can remove the host D3D12 device | Heap clamping to a reserved null slot, typed null descriptors matching shader annotations, indirect argument clamping, root SRV/UAV fixes, `ClearUnorderedAccessView*`, OOM mip-drop |
| **Secondary HLSL shader backend sync** | The secondary RDNA2 → HLSL → DXC backend (`xcl-core`) used for offline shader inspection and diagnostics fallback is being aligned with the latest tiling, BVH, and DXC fixes | Queued for workspace synchronization after GPU/shader validation |

## Workspace Architecture

See [docs/architecture.md](docs/architecture.md) for architectural internals and [docs/asset-policy.md](docs/asset-policy.md) for strict repository content rules.

| Crate / App | Responsibility |
|---|---|
| `victory-core` | Config resolution (`victory.toml` / env), asset-policy guard, mirrored `latest.log` tracing |
| `victory-pe` | Read-only PE32+ and import-library analyzer |
| `victory-loader` | In-process PE mapping, static TLS slots, crash stack unwinder, Zen 2 instruction helpers, runtime patching |
| `victory-hle` | Console API HLE (`kernelx`, `xgameruntime`, `dstorage`, `gameinput`, `audio`, `files` CoW VFS + lazy page support) |
| `victory-gpu` | D3D12.X objects, shadow/write-watched memory, indirect draws, Agility SDK init, frame capture & DRED diagnostics |
| `victory-shader` | Direct RDNA2 → SSA → DXIL bitcode translator + diagnostics probe/fallback bridge (`xcl_fallback`) |
| `victory-formats` | BCPack decompression, GFX10 texture layouts/detiling, XMA/Opus audio decoding |
| `victory-pipeline` | Content-addressed asset conversion cache |
| `victory-game` | Title format decoders (`sgd5`), build IDs, and hooks |
| `xcl-core` | Secondary RDNA2 → HLSL → DXC shader compiler, GFX10 tiling tables, BVH/UMA helpers, CoW VFS, Zen 2 decoder, RPF9 tools |
| `apps/xclvic` | Unified CLI entry point (`xclvic.exe`, built with `/HIGHENTROPYVA:NO` for `<2^40` console addressing) |
| `xtask` | Asset-policy enforcement (`cargo xtask policy`) and git hook installer (`cargo xtask install-hooks`) |

## Requirements & Asset Policy

- **Platform:** Windows 10/11 x64, AVX2-capable CPU, Direct3D 12 GPU with Shader Model 6.6 (Mesh Shaders required for world rendering), multi-GiB pagefile/commit headroom.
- **Toolchain:** Rust (`edition = "2024"`, `rust-version = "1.88"`).
- **Zero-asset policy (Rule #1):** Never commit proprietary executables, game archives, extracted shaders, textures, or screenshots. Run `cargo xtask policy` before committing and configure user data outside the repository.

## Build and Test

```powershell
# Verify asset policy
cargo xtask policy

# Check, build workspace, and build diagnostics profile
cargo check --offline --workspace --all-targets
cargo build --offline --workspace
cargo build --offline --profile diag -p xclvic --features diagnostics

# Run workspace unit tests (no proprietary title files required)
cargo test --offline --workspace -- --test-threads=4
```

Configure local title and cache directories outside the repository:

```powershell
copy victory.example.toml victory.toml
cargo run --offline -p xclvic -- config
```

## CLI Commands

```text
xclvic inspect <exe|dll> [--imports] [--exports]
xclvic census [paths...] [--missing] [--strict]
xclvic lib-exports <import.lib> [--list]
xclvic shader-info [paths...] [--verbose] [--disasm]
xclvic translate [paths...] [--validate] [--pipelines] [--list] [--jobs N] [--fallback]
xclvic disasm <file> [--offset N] [--length N] [--all] [--hex] [--fssm FILE]
xclvic shaders <file|folder|package.rpf> [--top N] [--json] [--recomp] [--compile N]
xclvic rpf <package.rpf> [--top N] [--find NAME] [--extract NAME --out DIR]
xclvic run [exe] [--execute] [--unimplemented trap|zero] [--strict] [--console series-x|series-s] [-- <title args>]
xclvic config
```

## Diagnostics & Environment Variables

Running with `--execute` writes automatic diagnostics under `%LOCALAPPDATA%\victory\data\debug\`:
- `latest.log` — live session tracing log
- `crashes.log` — unhandled exceptions and host GPU removal reports
- `frames\` (`diag` profile) — `latest.png`, startup frame, periodic captures, and black-frame captures

Common environment flags:

| Variable | Effect |
|---|---|
| `VICTORY_LOG` | Tracing level filter (e.g. `VICTORY_LOG=info,victory_gpu=debug`) |
| `VICTORY_D3D12_DEBUG=1` | Enable host D3D12 debug layer |
| `VICTORY_D3D12_GBV=1` | Enable GPU-based validation (slow, checks descriptor access) |
| `VICTORY_D3D12_DRED=1` | Enable DRED breadcrumbs and page-fault diagnostics |
| `VICTORY_D3D12_WARP=1` | Use Windows WARP software adapter (avoids GPU driver TDR resets) |
| `VICTORY_GPU_TIMING=1` | Per-list GPU timing and slowest command reporting |
| `VICTORY_SKIP_PIPELINES=<hash,...>` | Skip draws/dispatches referencing specific 16-digit shader hashes |
| `VICTORY_PIPELINE_BUDGET_MS` | Adaptive per-frame pipeline creation budget |
| `VICTORY_SHADER_LOOP_LIMIT` | Loop iteration safety limit for translated shaders |
| `VICTORY_CAPTURE`, `VICTORY_CAPTURE_FRAMES`, `VICTORY_CAPTURE_EVERY`, `VICTORY_DUMP_TARGETS` | Frame and render-target capture controls |
| `VICTORY_WATCH`, `VICTORY_CRASH_SAVE`, `VICTORY_STACKS`, `VICTORY_KEYS` | Memory watches, crash memory dumps, timed thread stacks, automated keys |
| `VICTORY_DXIL` | Override path to `dxil.dll` for `xclvic translate --validate` |
| `XCLVIC_PATCH` / `XCL_PATCH` | Runtime byte patches (`<module>+0x<rva>:<old_hex>=<new_hex>;...`) |
| `XCL_SPLIT_WAVE64`, `XCL_RECOMP_DXC`, `XCL_DUMP_HLSL` | Controls for `xcl-core` RDNA2 → HLSL → DXC tools |

## Contributing

1. Ensure `cargo xtask policy` passes and no personal paths or proprietary data are staged.
2. Develop changes on focused branches (`fix/...`, `feat/...`, `perf/...`, `docs/...`) and open Pull Requests against `main` documenting the root cause, affected crates, trade-offs, and test output.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).  
`crates/victory-formats/src/bcpack-tables.bin` carries the documented asset-policy exception in [docs/asset-policy.md](docs/asset-policy.md).
