# XBVK / XCLVIC — Xbox Series X|S Compatibility Layer

**XBVK (XCLVIC)** is an experimental native Windows compatibility layer built in Rust that runs Xbox Series X|S (`Scarlett`) x86-64 titles directly inside a host Windows process.

> **Graphics API architecture:** Translates console **D3D12.X** command streams and **RDNA2** shaders natively onto **Windows Direct3D 12 (Shader Model 6.6 / DXIL)** (not Vulkan).

## Current Status & Active Work

**Status: Pre-alpha (under active development; in-world 4fps, shaders works).**

### What works today
- **Loader & PE/TLS runtime (`victory-loader`, `victory-pe`):** Maps title executables and DLLs in-process (releasing raw PE buffers immediately after mapping to lower peak RAM), rewrites static TLS accesses, emulates console-only Zen 2 CPU instructions (`clzero`, etc.) via vectored exception handling, supports runtime hooking (`hooks`) and byte patching via `XCLVIC_PATCH`, and writes `module+rva` crash backtraces to `crashes.log`.
- **Direct3D 12.X runtime & memory (`victory-gpu`, `victory-pipeline`):** Translates console D3D12.X command lists, descriptors, indirect draws/dispatches, and shadow/write-watched title memory to Windows D3D12, with a 64 KiB page directory (`SLOT_MEMORY`) for raw 40-bit GPU addresses, UMA in-place heap probing (`OpenExistingHeapFromAddress*`), concurrent memory-mapped `ID3D12PipelineLibrary`, and DRED/breadcrumb diagnostics.
- **RDNA2 → DXIL shader compiler (`victory-shader`):** Direct RDNA2 → SSA → DXIL bitcode compiler featuring exact DXIL optimization passes (DCE, constant folding, value reuse), `image_bvh_intersect_ray` / `image_bvh64_intersect_ray` ray-tracing lowering, spilled compute user-data rewriting, Wave64-on-Wave32 compute emulation (`structure_wide`), loop safety caps, and D3D12 32 KiB mesh output validation.
- **Console HLE, formats & virtual filesystem (`victory-hle`, `victory-formats`):** Implements core `kernelx`, `xgameruntime`, `dstorage` (with structured error mapping), `gameinput`, `apu`/`audio`, `net`/`xnetworking`, `xmem`, `xstore`, `display`, BCPack/XMA/GFX10 texture decoders, and drive redirection (`G:` read-only with copy-on-write overlay, `T:` temp storage), with live mirrored logging to `%LOCALAPPDATA%\victory\data\debug\latest.log`.
- **Boot & UI flow:** Startup sequence, intro audio/video playback, offline profile/save persistence, and loading stages run up to world entry; diagnostics builds capture PNG frames (`latest.png`, boot capture, periodic captures, and black-frame anomalies) under `%LOCALAPPDATA%\victory\data\debug\frames\`.

### Known Issues & Ongoing Work
Active engineering work is tracked in GitHub Issues and focused on stabilizing world rendering, shader compatibility, and GPU execution stability:

| Area | Issue | Landed in `main` | Remaining Work |
|---|---|---|---|
| **First-frame shader stalls & PSO hitching** | [#3](https://github.com/0x0007b/XBVK/issues/3) | Concurrent memory-mapped `ID3D12PipelineLibrary` (`VICTORY_LIBRARY_MAP`), DXIL bitcode optimizer (DCE/fold/reuse), disk shader cache | MRU early-warmup pipeline list, adaptive per-frame compile budgets (`VICTORY_PIPELINE_BUDGET_MS`), RAM-bounded worker scaling |
| **Driver TDR timeouts (`DXGI_ERROR_DEVICE_HANG`)** | [#4](https://github.com/0x0007b/XBVK/issues/4) | `VICTORY_SHADER_LOOP_LIMIT` safety cap, DRED & breadcrumb diagnostics | Per-list command submission splitting (`VICTORY_LIST_SPLIT`), mesh dispatch budgeting (`VICTORY_MESH_BUDGET`), shader spin-wait cap (`VICTORY_SHADER_SPIN_LIMIT`) |
| **Wave64 shaders on Wave32 GPUs & Mesh limits** | [#5](https://github.com/0x0007b/XBVK/issues/5) | Compute Wave64-on-Wave32 emulation (`structure_wide`, `VICTORY_HOST_WAVE=32`, host test suite), D3D12 32 KiB mesh output validation | Mesh stage Wave64 emulation (`emulate_wave64`), automatic output clamping, uniform `GS_ALLOC_REQ` broadcast |
| **Texture readbacks & RT/UAV formats** | [#6](https://github.com/0x0007b/XBVK/issues/6) | `ALLOW_RENDER_TARGET` guard in `Host::rtv` to skip invalid RTV creation | Full `CopyTextureRegion` texture-to-buffer readback staging, mismatched texel-size guards, fixing `0xc0000409` test failure in readback staging |
| **Descriptors, root views, indirect & `xcl-core` sync** | [#7](https://github.com/0x0007b/XBVK/issues/7) | `VICTORY_VIEW_LIMIT` heap cap, raw address memory directory, UMA in-place probing, shadow/descriptor/indirect hardening | Typed null descriptors for empty slots, `ClearUnorderedAccessView*`, OOM mip-drop fallback, `xcl-core` tiling/BVH/UMA/DXC sync |

## Workspace Architecture

See [docs/architecture.md](docs/architecture.md) for architectural internals and [docs/asset-policy.md](docs/asset-policy.md) for strict repository content rules.

| Crate / App | Responsibility |
|---|---|
| `victory-core` | Config resolution (`victory.toml` / env), asset-policy guard, mirrored `latest.log` tracing |
| `victory-pe` | Read-only PE32+ and import-library analyzer |
| `victory-loader` | In-process PE mapping, static TLS slots, crash stack unwinder, Zen 2 console instruction emulation (`console_insn`), runtime hooks & patching |
| `victory-hle` | Console API HLE (`kernelx`, `xgameruntime`, `dstorage`, `gameinput`, `apu`/`audio`, `net`/`xnetworking`, `xmem`, `xstore`, `display`, `files` CoW VFS) |
| `victory-gpu` | D3D12.X objects, shadow/write-watched memory, 40-bit address memory directory (`memory`), UMA in-place heap probe (`inplace`), memory-mapped pipeline library, indirect draws, frame capture & DRED diagnostics |
| `victory-shader` | Direct RDNA2 → SSA → DXIL bitcode translator, DXIL bitcode optimizer, BVH ray-tracing lowering, spilled user-data rewriter, Wave64-on-Wave32 compute lowering + fallback bridge (`xcl_fallback`) |
| `victory-formats` | BCPack decompression/encoding, GFX10 texture layouts/detiling + layout benchmark, XMA/Opus audio decoding |
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
| `VICTORY_SHADER_LOOP_LIMIT` | Loop iteration safety limit for translated shaders |
| `VICTORY_HOST_WAVE=32` | Translate shaders targeting a host GPU with max wave size 32 (enables Wave64-on-Wave32 compute lowering) |
| `VICTORY_VIEW_LIMIT=<N>` | Translate shaders assuming a capped host descriptor view heap slot limit |
| `VICTORY_LIBRARY_MAP=0`, `VICTORY_LIBRARY_HANDLE=0`, `VICTORY_LIBRARY_LOCK=1` | Fallback toggles for `ID3D12PipelineLibrary` memory mapping, file-handle retention, and serialized loading |
| `VICTORY_INPLACE_PROBE=1` | Probe host D3D12 driver support for `OpenExistingHeapFromAddress*` / UMA in-place heaps |
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
