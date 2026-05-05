# Design: ARM64 build support for the Rust crates

**Status:** Draft (2026-05-05)

## Goal

Make every `nvda_*` Rust crate build for ARM64 in addition to x86_64, and link the resulting libraries into the existing ARM64 NVDA build. Drop the per-port `#ifdef _M_X64` C++ fallbacks once both archs route through Rust on the relevant call paths.

## Non-goals

* Adding 32-bit (`x86`) Rust support. NVDA's `x86` build is the legacy `synthDriverHost32` shim; nothing in our `nvda_*` crates needs to load there.
* CI for ARM64. The user wants to opt in once the build works locally.
* Refactoring the Rust crates themselves -- the libraries are already cross-arch via windows-rs.

## Background — what already works

* **C++ side, ARM64**: `SConstruct:241` clones `envArm64` with `TARGET_ARCH="arm64"` and runs `archBuild_sconscript` with `variant_dir="build/arm64"`. The C++ helper DLLs already build for ARM64 today; they just compile the C++ `#else` branches (the verbatim originals) for every helper we've ported.
* **Cargo workspace**: `windows-rs` 0.58 is cross-arch; the Rust source has no `#[cfg(target_arch = "x86_64")]` -- everything compiles for ARM64 once the toolchain target is installed.
* **Python ARM64 support**: `pyproject.toml` requires `>=3.13,<3.14` and `uv` is used for env management. NVDA already ships ARM64 builds with an ARM64 Python interpreter; the maturin extension just hasn't been wired up for it.

## What's missing

Three pieces, in order of dependency:

1. **`nvda_input_hooks` and `nvda_ia2` static libs for ARM64**: the SCons block in `nvdaHelper/remote/sconscript:130-194` is currently gated `if isX64:` and invokes cargo without a `--target` flag (so it builds for the host triple). We need to broaden the gate to also fire for `arm64` and pass `--target aarch64-pc-windows-msvc` when cross-compiling.
2. **`nvdaRust` Python extension for ARM64**: `SConstruct:558-617`'s `buildNvdaRust` calls `uvx maturin develop`, which builds for whatever Python interpreter `uvx` chose. For an ARM64 NVDA build the .pyd has to be ARM64-native; on a non-ARM64 host that means cross-compilation via `maturin build --target aarch64-pc-windows-msvc -i <arm64-python>`.
3. **Drop `#ifdef _M_X64` fallbacks**: every C++ delegation we've shipped (PR 1-5 + getSelectedItem + IAccessible2FromIdentifier + WASAPI) has the form `#ifdef _M_X64 ... call Rust shim ... #else ... verbatim C++ ... #endif`. Once ARM64 routes through Rust too, the `#ifdef` is conditionally dead. Replace with a single project-level macro and prune the verbatim C++ for files that are fully ported.

Each piece is independent enough to land as its own PR.

## Architecture

### Piece 1: cargo for ARM64

`nvdaHelper/remote/sconscript:92,132-194` contains the cargo-build command. Current shape:

```python
isX64 = env["TARGET_ARCH"] == "x86_64"
...
if isX64:
    rustTargetDir = Dir("#build/rust")
    inputHooksLib = rustTargetDir.File("release/nvda_input_hooks.lib")
    ia2Lib = rustTargetDir.File("release/nvda_ia2.lib")
    ...
    def buildCargoStaticLibs(target, source, env):
        result = subprocess.run([
            "cargo", "build", "--release",
            "--package", "nvda_input_hooks",
            "--package", "nvda_ia2",
            "--target-dir", rustTargetDir.abspath,
            "--manifest-path", rustWorkspaceDir.File("Cargo.toml").abspath,
        ], ...)
```

Two changes:

1. **Broaden the gate**: `isRustArch = env["TARGET_ARCH"] in ("x86_64", "arm64")`. The 32-bit and arm64ec variants don't get Rust libs (they're not on the path that loads our COM helpers).
2. **Pass `--target` and adjust output paths**: when `TARGET_ARCH == "arm64"`, append `--target aarch64-pc-windows-msvc` to the cargo invocation and prefix the output paths with `aarch64-pc-windows-msvc/`. For host-arch builds (x86_64 on x64 host) we can keep the current "no `--target`" behaviour, OR normalise both archs to use `--target` so paths are predictable. **Decision:** normalise to always use `--target` -- that way the SCons-managed lib path is always `<rustTargetDir>/<triple>/release/<lib>` regardless of arch, and cross-vs-native cargo behaviour is consistent.

The triple lookup:

| `TARGET_ARCH` | `isArm64EC` | Cargo target triple |
| --- | --- | --- |
| `x86_64` | n/a | `x86_64-pc-windows-msvc` |
| `arm64` | `False` | `aarch64-pc-windows-msvc` |
| `arm64` | `True` | `arm64ec-pc-windows-msvc` |

After the change:

```python
isRustArch = env["TARGET_ARCH"] in ("x86_64", "arm64")
RUST_TARGET_TRIPLE = {
    "x86_64": "x86_64-pc-windows-msvc",
    "arm64": "aarch64-pc-windows-msvc",
}
if isRustArch:
    triple = RUST_TARGET_TRIPLE[env["TARGET_ARCH"]]
    archDir = f"{triple}/release"
    inputHooksLib = rustTargetDir.File(f"{archDir}/nvda_input_hooks.lib")
    ia2Lib = rustTargetDir.File(f"{archDir}/nvda_ia2.lib")
    ...
    cargo_cmd = [
        "cargo", "build", "--release",
        "--target", triple,
        "--package", "nvda_input_hooks",
        "--package", "nvda_ia2",
        ...
    ]
```

**Toolchain prerequisite**: `rustup target add aarch64-pc-windows-msvc` must be run on each dev machine that builds ARM64. We document this in the README's build-prereqs section. A graceful-degradation check (`rustc --print target-list | grep aarch64`) before invoking cargo would surface the missing toolchain with a clear error.

### Piece 2: maturin for ARM64

The Python extension is harder than the static libs because maturin builds against a specific Python interpreter ABI, not just an arch. Three viable shapes:

**Option A — host-only build, no ARM64 nvdaRust**: leave `buildNvdaRust` x64-only for now. ARM64 NVDA falls back to whatever pre-Rust C++ paths still exist. Where there's no fallback (e.g. `nvdaRust.wasapi` is the only audio implementation post-PR-d1e210afc), ARM64 NVDA simply doesn't work. **Reject** -- we want ARM64 NVDA to work post-port.

**Option B — native ARM64 build only, run on ARM64**: dev/CI on an ARM64 host installs an ARM64 Python (`uv python install 3.13 --python-platform win-arm64`), creates an ARM64 venv, and `uvx maturin develop` Just Works. Cross-compilation is not attempted; you build ARM64 binaries on ARM64 machines. This matches the cargo step's "host arch" behaviour pre-`--target`. **Pro**: simplest setup. **Con**: x64 dev machines can't build ARM64 NVDA, and the friend with the ARM64 VM has to do the maturin step.

**Option C — cross-compile maturin from x64**: `maturin build --target aarch64-pc-windows-msvc -i <arm64-python>` produces a wheel; SCons unpacks the wheel into the target site-packages. Requires:

* An ARM64 Python install on the build host (uv handles this: `uv python install 3.13 --python-platform win-arm64`).
* `PYO3_CROSS_PYTHON_VERSION=3.13` and `PYO3_CROSS_LIB_DIR=<arm64-python>/Lib` env vars, OR pyo3's `--target` cross-compile mode (auto-detected when `--target` is set and an ARM64 python is reachable).
* `maturin build` instead of `maturin develop`, then a manual install step.

**Decision:** Option B for now. It matches the cargo step's existing convention (build artifacts are always for the build host arch unless we go out of our way). Document the workflow: "ARM64 builds happen on an ARM64 machine; cross-compilation from x64 is a future optimisation if it becomes painful." The friend's ARM64 VM is the canonical ARM64 build host.

In the SCons code, this means: when `TARGET_ARCH == "arm64"` we still call `uvx maturin develop`, just from the ARM64 venv. The build host's `uv` is expected to have selected the ARM64 Python (because the `pyproject.toml` `requires-python = ">=3.13,<3.14"` matches whatever ARM64 Python the user has). No SCons code changes for the maturin step itself; only its trigger gate may change.

A small wart: the `nvdaRustPyd` target file path embeds the platform tag (`cp313-win_amd64` vs `cp313-win_arm64`). Already correct -- `_sysconfig.get_platform()` returns the arch-specific tag.

### Piece 3: drop `#ifdef _M_X64` fallbacks

Currently every C++ port we've done has the shape:

```cpp
#ifdef _M_X64
extern "C" { void* nvda_ia2_foo(...); }
... thin C++ wrapper calling the shim ...
#else
... original verbatim C++ ...
#endif
```

Once piece 1 lands, the Rust libs link on ARM64 too. The `#ifdef _M_X64` should be replaced with `defined(_M_X64) || defined(_M_ARM64)` or a single project macro `NVDA_HAS_RUST_HELPERS` defined in both x64 and arm64 archBuild_sconscript invocations.

**Decision:** introduce `NVDA_HAS_RUST_HELPERS` as a `CPPDEFINES` entry set conditionally in `nvdaHelper/archBuild_sconscript` based on `isRustArch`. New ports henceforth use this macro. Existing `#ifdef _M_X64` blocks get migrated as part of the same PR.

For the `#else` verbatim C++: **drop it** for files where every Rust port already has the same arch coverage as the new macro. If a future port lands without ARM64 support (unlikely), we re-introduce a fallback. Removing dead C++ shrinks the binary and reduces drift risk between the Rust port and the C++ original.

| File | Has any deferred function? | Action |
| --- | --- | --- |
| `nvdaHelper/common/ia2utils.cpp` | yes (`getAccessibleChildren`) | keep `#else` only for the deferred bits |
| `nvdaHelper/remote/textFromIAccessible.cpp` | no | drop `#else` |
| `nvdaHelper/remote/ia2LiveRegions.cpp` | no | drop `#else` |
| `nvdaHelper/remote/IA2Support.cpp` | yes (lots) | keep `#else` for un-ported functions |
| `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp` | yes (most of the file) | keep `#else` until fully ported |

## File structure

**Modify (piece 1):**

| File | Change |
| --- | --- |
| `nvdaHelper/remote/sconscript` | broaden `isX64` gate to `isRustArch`, pass `--target` to cargo, normalise lib paths to include the triple |
| `readme.md` (or `projectDocs/dev/buildingNVDA.md` if that's the canonical build-prereq doc) | add `rustup target add aarch64-pc-windows-msvc` to the prereqs |

**Modify (piece 3):**

| File | Change |
| --- | --- |
| `nvdaHelper/archBuild_sconscript` | add `NVDA_HAS_RUST_HELPERS` to `CPPDEFINES` when `TARGET_ARCH in ("x86_64", "arm64")` |
| Each `*.cpp` we've ported | replace `#ifdef _M_X64` with `#ifdef NVDA_HAS_RUST_HELPERS`; drop `#else` for fully-ported files |

## Testing

**Piece 1**:

* Local x64 build still produces an identical-behaviour `nvdaHelperRemote.dll` (maybe a slightly different binary because the cargo `--target` flag may change rustc's default codegen settings; verify by running existing smoke tests).
* The friend builds on ARM64 VM: `scons source` should produce `source/lib/arm64/nvdaHelperRemote.dll` containing the Rust shims, and ARM64 NVDA should run with no `nvda_ia2` panics in the log.
* Smoke tests: re-run the manual smoke tests from the recent porting PRs (Firefox heading nav, link reading, live regions, caret nav, audio output) on the ARM64 build.

**Piece 2**: ARM64 NVDA loads and `nvdaRust` is importable. Audio output works (this is the only critical user of `nvdaRust` today since `nvwave.py` requires it).

**Piece 3**: existing x64 binaries should be byte-identical to before piece 3 (the macro substitution doesn't change x64 codegen). ARM64 binaries should now contain Rust call paths.

## Commit plan

1. Generalise `nvdaHelper/remote/sconscript` cargo block to support ARM64 + add `--target` for x64 too.
2. Document the rustup target prereq in the build-prereqs doc.
3. Introduce `NVDA_HAS_RUST_HELPERS` and migrate existing `#ifdef _M_X64` to it; drop `#else` blocks for fully-ported files.

PR carve-up: commits 1-2 land as "ARM64 cargo support" (mechanical, no behaviour change on x64). Commit 3 lands as a follow-up ("Drop `_M_X64` fallbacks now that ARM64 supports Rust") once the friend has confirmed the ARM64 build works.

## arm64ec

NVDA's SCons builds the helper DLLs unconditionally for every arch in `archBuild_sconscript:253`, so **arm64ec currently links the C++ verbatim `#else` branches** in every helper we've ported. If we drop the `#else` blocks in piece 3 without thinking about arm64ec, the arm64ec build of `nvdaHelperRemote.dll` loses those symbols. So either we keep the `#else` blocks or we bring arm64ec into the Rust fold.

**Status of `arm64ec-pc-windows-msvc` as of 2026-05-05:**

* **Tier 2** stable, available via `rustup target add arm64ec-pc-windows-msvc`. std is supported; static and dynamic libs build. (Promoted from Tier 3 in 2024.)
* The target presents as x86_64 to the OS, has its own name mangling, requires entry/exit thunks for some functions, and uses a different call-checker. LLVM 18.1.4+ required (any current rustc satisfies this).
* **Known open issue ([rust-lang/rust#131172](https://github.com/rust-lang/rust/issues/131172))**: arm64ec sets `target_arch = "arm64ec"` rather than `"aarch64"`. Code using `#[cfg(target_arch = "aarch64")]` won't fire on arm64ec. Our `nvda_*` crates have no such gates but **windows-rs internals might** -- this is the main probe-test risk.
* **PyO3 / maturin / arm64ec**: no issues filed in either repo. PyO3 uses raw-dylib for Python linkage, which should work for any Tier 2 target -- but untested in practice for arm64ec.

**Probe result (2026-05-05):** ran `cargo build --release --target arm64ec-pc-windows-msvc --package nvda_ia2 --package nvda_input_hooks` against windows-rs 0.58 with the existing source. Build succeeds clean; windows-rs routes arm64ec through its `windows_x86_64_msvc` link layer (consistent with arm64ec's x64-ABI outside-the-binary contract). `target_arch = "arm64ec"` quirk does not affect us because our crate uses no `cfg(target_arch=...)` gates and windows-rs's internal gating evidently handles the case correctly. aarch64-pc-windows-msvc also builds clean.

**Updated decision for piece 1:** include arm64ec alongside arm64 in the cargo step. Both targets build the static libs; both link into their respective `nvdaHelperRemote.dll` variants. The `RUST_TARGET_TRIPLE` table grows by one row.

**Updated decision for piece 2 (maturin)**: keep arm64ec OUT of scope. arm64ec Python extensions via PyO3 are uncharted; defer until someone has a concrete reason to need a Rust-built `nvdaRust.pyd` for arm64ec processes (which today is a small slice of the user base on top of an already-small ARM64 slice). The arm64ec NVDA build can use whatever non-Rust audio/text path NVDA had pre-Rust... wait, that's not actually an option since `nvwave.py` requires `nvdaRust.wasapi`. **Revised:** the arm64ec NVDA build needs to load an arm64ec `nvdaRust.pyd`, OR the arm64ec Python in NVDA's bundle needs to load an x64 `nvdaRust.pyd` via x64 emulation. The latter is plausible (arm64ec is built for exactly this kind of cross-arch code in one process) but needs to be verified before committing to either path. **Action:** bump this to a proper open question; treat piece 2 + arm64ec as a separate investigation.

**Updated decision for piece 3:**

* Replace `#ifdef _M_X64` with `#ifdef NVDA_HAS_RUST_HELPERS` ✓
* Define `NVDA_HAS_RUST_HELPERS` for both `TARGET_ARCH == "x86_64"` and `TARGET_ARCH == "arm64"` (covering both arm64 and arm64ec, since both link the Rust libs per the probe above).
* `#else` C++ verbatim can now be safely dropped for fully-ported files because every arch that compiles `nvdaHelperRemote.dll` also routes through Rust.

## Open questions

* **arm64ec maturin / PyO3**: can we build `nvdaRust.pyd` for arm64ec? Or does the arm64ec NVDA bundle need to load an x64 `nvdaRust.pyd` via emulation? Non-trivial investigation -- defer until arm64 (non-EC) support is shipped and we have a concrete need.
* **Maturin cross-compile (arm64 from x64 host)**: piece 2 deliberately punts. If x64-host-builds-ARM64-NVDA becomes a workflow we want to support (e.g. for our own CI on x64 runners, or for releasing), revisit Option C with `PYO3_CROSS_LIB_DIR`. For now, ARM64 builds happen on ARM64 hosts.
* **CI**: turning on ARM64 in `.github/workflows/testAndPublish.yml` (`supportedArchitectures: '["x64"]'` -> `'["x64", "arm64"]'`) doubles runner-minute usage. Out of scope for this design; the user can flip the switch when they're comfortable with the runner-minute cost.
