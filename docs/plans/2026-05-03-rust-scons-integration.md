# Move nvdaRust build from uv workspace to SCons — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make SCons the authoritative builder for `nvdaRust.pyd` so production builds (`scons launcher`) always get a fresh release-mode wheel, dev builds (`scons source`) get a fast cargo-dev-profile wheel matching the C++ `/Od` convention, and the uv wheel-cache trap (which silently shipped a stale `.pyd` to NVDA earlier today) is eliminated.

**Architecture:** Drop `rust/nvda_python` from the uv workspace and the project's dependency groups. Add a new SCons `Command` target that invokes `uvx maturin develop` to build and install `nvdaRust` into `.venv/Lib/site-packages/nvdaRust/`. Profile selection mirrors the C++ convention exactly: SCons `release` BoolVariable false (default) → cargo `dev` profile (`maturin develop`); `release` true → cargo `release` profile (`maturin develop --release`). The `dist` target depends on the Rust install so `scons launcher` is always production-correct. `runnvda.bat` switches to `uvw run --no-sync` to stop uv from re-managing nvdaRust on launch. Imports from `uv run python -c "import nvdaRust"` continue to work because Python's import machinery finds the SCons-installed `.pyd` in site-packages — uv leaves un-managed packages alone.

**Tech Stack:** SCons (existing build system), maturin via `uvx`, cargo. No new dependencies.

---

## Why this change?

Today's session surfaced two real failures of the current uv-workspace approach for `nvdaRust`:

1. **The wheel-cache trap.** uv's wheel cache for workspace members keys on `pyproject.toml` + lockfile, NOT on the underlying Rust source files. After we modified `rust/nvda_python/src/lib.rs` (adding pyo3-log), `uv sync` kept reusing a stale wheel built before the change. `nvdaRust.cp313-win_amd64.pyd` in `.venv` was 8+ hours stale even after multiple `uv sync` invocations. Cleaning the cache (`uv cache clean`) was needed to force a rebuild — that's not a workflow we can ship to other contributors.
2. **No production fail-safe.** `scons launcher` runs `setup.py` with py2exe, which bundles whatever `.pyd` is currently in `.venv/Lib/site-packages/`. If the dev workflow last ran `uvx maturin develop` (dev profile), the installer ships a dev-build `.pyd`. If a stale wheel is cached (case #1), the installer ships stale code. There's no SCons builder for Rust, no dependency tracking, no profile guarantee.

C++ doesn't have these problems because `scons source` is the single entry point: dependency tracking is mtime-based and works on `.cpp`/`.h` directly, profile is selected by the `release` BoolVariable, production builds always rebuild from source. This plan brings Rust into that same model.

The previous SCons-built integration existed in commit `fe6c65c89` ("build: integrate Rust/maturin build into SCons") and was removed by `b41cdd8f7` ("refactor: make nvdaRust a uv workspace member instead of SCons-built") for dev-convenience reasons. We now have two more crates (`nvda_input_hooks` already SCons-built as a staticlib for `nvdaHelperRemote.dll`) and concrete data showing the dev convenience cost real correctness. The decision to revert is informed by that data.

---

## Scope

**In scope:**

* SCons builder for `nvda_python` (the cdylib that becomes `nvdaRust.pyd`).
* Profile selection wired to SCons `release` BoolVariable.
* Removal of `rust/nvda_python` from uv workspace + sources + dev deps.
* `runnvda.bat` change to `--no-sync`.
* Documentation update to reflect the new dev workflow.
* Verification that `import nvdaRust` continues to work in `uv run`.

**Out of scope:**

* Refactoring the existing `nvda_input_hooks` SCons cargo block. It's already SCons-built (different artifact: staticlib for `nvdaHelperRemote.dll`); keep as-is.
* Any change to how `cargo` itself manages the workspace at `rust/Cargo.toml`. The cargo workspace is independent of the uv workspace.
* Multi-arch `nvdaRust.pyd`. Python extensions match the host Python's arch; we build only that arch (currently x86_64). NVDA's other arches (x86, arm64, arm64ec) bundle their own Python with their own `nvdaRust.pyd` built at install/release time per arch — out of scope here, single-arch host suffices for dev.
* `[tool.maturin]` profile config in `rust/nvda_python/pyproject.toml`. We pass profile via the `--release` flag to `maturin develop` instead — keeps dev/release selection in SCons.

---

## File Structure

**Modify:**

* `sconstruct` — add `nvdaRustInstall` SCons `Command` target after the existing nvdaHelper SConscript invocations; make `dist` depend on it.
* `pyproject.toml` — remove `nvdaRust = { workspace = true }` from `[tool.uv.sources]`, remove `"rust/nvda_python"` from `[tool.uv.workspace] members`, remove `"nvdaRust"` from `[dependency-groups] dev`.
* `runnvda.bat` — add `--no-sync` flag to the `uvw run` invocation.
* `docs/plans/2026-05-02-rust-input-hooks-derisk.md` — append a brief note in "Notes for future ports" mentioning the architecture change so future readers don't get confused about why nvda_python is SCons-built but nvda_input_hooks lives next to other SCons cargo blocks.

**No new files. No deletions.** Existing `rust/nvda_python/Cargo.toml` and `pyproject.toml` (the maturin config) stay — `maturin develop` still needs them.

---

## Working assumptions

1. **`maturin develop` writes to the active virtualenv based on `VIRTUAL_ENV`.** When `scons` runs via `scons.bat` → `ensureuv.ps1` → `uv run --directory <repo> SCons`, the env has `VIRTUAL_ENV=<repo>/.venv`. So `uvx maturin develop` from any cwd inside the repo installs into the repo's `.venv`. Verify on first run by inspecting `.venv/Lib/site-packages/nvdaRust/` mtime after a SCons build.
2. **`uv sync` does NOT remove unmanaged packages from `.venv`.** Once nvdaRust is dropped from the uv workspace, uv should leave the SCons-installed `.pyd` alone. If it doesn't (verify with `uv sync` after the SCons install — does the .pyd vanish?), we have a problem and need to add a workaround. The plan includes an explicit verification step.
3. **`maturin develop` is idempotent and cheap when nothing changed.** Cargo's incremental build handles staleness; a no-op rebuild takes ~1s. SCons's mtime-based dependency tracking means we only invoke `maturin develop` when a Rust source file changed, so the steady-state cost is zero.
4. **Single-arch is enough.** Python extensions are arch-specific (the .pyd's filename includes the arch — `nvdaRust.cp313-win_amd64.pyd`). NVDA's per-arch installers (x86, arm64) build their own Python and would need their own per-arch `.pyd`; this plan handles only the dev/host arch (x86_64 currently). Other-arch builds are a separate cross-compilation concern.

---

## Task 1: Add SCons builder for nvdaRust

**Files:**

* Modify: `sconstruct`

* [ ] **Step 1: Read the current sconstruct cargo invocation pattern**

The existing nvda_input_hooks staticlib build in `nvdaHelper/remote/sconscript` (commit `46b6ddd93`) shows the cargo+SCons integration pattern that already works. Read it to understand the conventions:

```
sed -n '85,135p' nvdaHelper/remote/sconscript
```

Note the pattern:

* Inline `import os` / `import subprocess` (with `# noqa: E402`)
* `Dir("#rust/...")` syntax for path-from-repo-root
* Glob `*.rs` + `Cargo.toml` files for SCons dependency tracking
* `env.Command(target, source, action_callback)` for the actual build

Our `nvdaRust` build follows the same pattern but uses `maturin develop` (which installs to venv) instead of producing a `.lib` for linking.

* [ ] **Step 2: Find the right insertion point in sconstruct**

In `sconstruct`, find the line that defines `dist`:

```
grep -n "^dist = env.NVDADist" sconstruct
```

Expected: `527:dist = env.NVDADist("dist", [sourceDir, userDocsDir], uiAccess=...)`.

We'll insert the nvdaRust build target ABOVE that line so `dist` can depend on it.

* [ ] **Step 3: Insert the nvdaRust build target**

Insert this block immediately before the `dist = env.NVDADist(...)` line in `sconstruct`:

```python
# Build nvdaRust (the Rust PyO3 cdylib) via maturin and install it into the
# active virtualenv. We invoke `uvx maturin develop` so the .pyd lands in
# .venv/Lib/site-packages/nvdaRust/ where Python's import machinery finds it,
# and uv (no longer managing nvdaRust as a workspace member) leaves it alone.
#
# Profile selection mirrors C++: scons `release=False` (the default) → cargo
# dev profile (no `--release` flag); `release=True` → cargo release profile.
# This matches the /Od (dev) vs /O2 /GL /LTCG (release) distinction in
# nvdaHelper/archBuild_sconscript.
import subprocess as _subprocess  # noqa: E402

rustNvdaPythonDir = Dir("#rust/nvda_python")
rustWorkspaceDir = Dir("#rust")
# Glob every .rs and Cargo.toml across the workspace so SCons re-runs maturin
# whenever any Rust source or manifest changes (path-deps included).
nvdaRustSources = (
	env.Glob("#rust/nvda_*/src/*.rs")
	+ env.Glob("#rust/nvda_*/Cargo.toml")
	+ [rustWorkspaceDir.File("Cargo.toml")]
)

# Target is the installed .pyd inside .venv. Filename includes the Python
# major.minor and the platform tag — both are constants for a given build host.
import sysconfig as _sysconfig  # noqa: E402
_pyTag = f"cp{_sysconfig.get_python_version().replace('.', '')}"
_platTag = _sysconfig.get_platform().replace("-", "_")
nvdaRustPyd = File(
	f"#.venv/Lib/site-packages/nvdaRust/nvdaRust.{_pyTag}-{_platTag}.pyd",
)


def buildNvdaRust(target, source, env):
	"""Run `uvx maturin develop` to build and install nvdaRust.

	Profile is chosen by the SCons `release` BoolVariable — same convention
	as the C++ build (see nvdaHelper/archBuild_sconscript:133-139).
	"""
	cmd = [
		"uvx",
		"maturin",
		"develop",
		"--manifest-path",
		rustNvdaPythonDir.File("Cargo.toml").abspath,
	]
	if env["release"]:
		cmd.append("--release")
	result = _subprocess.run(cmd, capture_output=True, text=True)
	if result.returncode != 0:
		print(f"maturin develop failed:\n{result.stderr}")
		return result.returncode
	if not os.path.exists(target[0].abspath):
		print(
			f"maturin develop succeeded but {target[0].abspath} was not produced",
		)
		return 1
	return 0


nvdaRustInstall = env.Command(
	nvdaRustPyd,
	nvdaRustSources,
	buildNvdaRust,
)
# Always reconsider the build (maturin's own staleness check is fast); SCons's
# glob-based source list catches the common case of "I edited a .rs file".
AlwaysBuild(nvdaRustInstall)
```

**Engineer notes:**

* The `_pyTag`/`_platTag` derivation gives us `cp313-win_amd64` on the current build host. Hardcoding would also work but breaks if NVDA ever updates Python or supports a new arch.
* `AlwaysBuild` is intentional. SCons's glob-based source detection is good but maturin's own staleness check (cargo incremental + maturin's wheel hash) is the authoritative one. We let SCons consider the target stale every run; maturin then no-ops if nothing actually needs rebuilding (~1s overhead).
* We capture maturin's stderr for error reporting, drop stdout (it's noisy progress).
* `import os` is already imported at the top of `sconstruct`; `import subprocess` and `import sysconfig` are not — hence the `# noqa: E402` for "non-top-of-file import" since we want them locally scoped to this block.

* [ ] **Step 4: Make `dist` depend on the nvdaRust install**

Find the line `dist = env.NVDADist(...)` and IMMEDIATELY AFTER it (before `env.Depends(dist, uninstaller)` and other existing depends), add:

```python
env.Depends(dist, nvdaRustInstall)
```

This ensures `scons launcher` (which depends on `dist`) always gets a freshly-built nvdaRust before py2exe runs.

* [ ] **Step 5: Verify a dev build works**

Run from the project root:

```
./scons.bat source --all-cores 2>&1 | tail -10
```

Expected: build completes successfully; the line `[buildNvdaRust output ignored — only stderr captured on failure]` is silent (no print). Verify the `.pyd` was installed:

```
ls -la .venv/Lib/site-packages/nvdaRust/nvdaRust.cp313-win_amd64.pyd
```

Expected: mtime is "now" (within the last few seconds), size is 1-2 MB (dev build, larger than release).

If `maturin develop` fails (e.g., complains about missing VIRTUAL_ENV), check that `scons.bat` correctly invokes uv with the right env. The fix is upstream in `scons.bat`/`ensureuv.ps1`, NOT in this builder.

* [ ] **Step 6: Verify a release build works**

```
./scons.bat source --all-cores release=1 2>&1 | tail -10
```

Expected: same success; resulting `.pyd` should be smaller (~500-700 KB, release profile inlines/strips).

* [ ] **Step 7: Commit**

```bash
git add sconstruct
git commit -m "sconstruct: build nvdaRust via maturin (SCons-driven, profile-aware)"
```

---

## Task 2: Drop nvdaRust from the uv workspace

**Files:**

* Modify: `pyproject.toml`

* [ ] **Step 1: Remove nvdaRust from `[tool.uv.sources]`**

In `pyproject.toml`, find:

```toml
[tool.uv.sources]
nvda-misc-deps = { workspace = true }
configobj = { git = "https://github.com/DiffSK/configobj", rev = "9c8a0a80c767bf8a3d6493ed01df6c351bddca42" }
nvda-mathcat = { workspace = true }
nvdaRust = { workspace = true }
```

Delete the `nvdaRust = { workspace = true }` line.

* [ ] **Step 2: Remove nvdaRust from `[tool.uv.workspace]` members**

Find:

```toml
[tool.uv.workspace]
members = [
	"miscDeps",
	"include/nvda-mathcat",
	"rust/nvda_python",
]
```

Delete the `"rust/nvda_python",` line.

* [ ] **Step 3: Remove nvdaRust from `[dependency-groups] dev`**

Find the `dev = [...]` block (under `[dependency-groups]`). Delete the `"nvdaRust",` line.

* [ ] **Step 4: Verify uv still parses the project**

```
uv sync --dry-run 2>&1 | tail -5
```

Expected: no errors. uv may or may not list `nvdaRust` as "would remove" depending on its tracking — note if it does, we'll handle it in Task 4.

* [ ] **Step 5: Commit**

```bash
git add pyproject.toml
git commit -m "pyproject: drop nvdaRust from uv workspace (now SCons-built)"
```

---

## Task 3: Verify uv leaves the SCons-installed nvdaRust alone

**Files:** none.

* [ ] **Step 1: Confirm the .pyd is present from Task 1's build**

```
ls -la .venv/Lib/site-packages/nvdaRust/nvdaRust.cp313-win_amd64.pyd
```

Expected: file exists, mtime from Task 1 Step 5/6.

* [ ] **Step 2: Run `uv sync` (the un-cached, normal path) and check the .pyd survives**

```
uv sync 2>&1 | tail -5
ls -la .venv/Lib/site-packages/nvdaRust/nvdaRust.cp313-win_amd64.pyd
```

Expected:

* `uv sync` runs without errors.
* The `.pyd` is still present with the same mtime as before.

If the `.pyd` was removed: uv is more aggressive about cleaning unmanaged packages than expected. Workarounds, in order of preference:

1. Add a stub `.dist-info` directory that uv recognizes (non-trivial).
2. Mark `nvdaRust` as `[[tool.uv.dependency-metadata]]` with empty install requirements (untested).
3. Re-add nvdaRust as a uv workspace member with `[tool.uv] no-sources = true` and a no-op build backend (non-trivial).

Document which path was needed (if any) and proceed.

* [ ] **Step 3: Run `uv sync --reinstall`** (the more aggressive path) and check again

```
uv sync --reinstall 2>&1 | tail -5
ls -la .venv/Lib/site-packages/nvdaRust/nvdaRust.cp313-win_amd64.pyd
```

Expected: same — `.pyd` survives. If not, escalate.

* [ ] **Step 4: Verify Python imports still work**

```
uv run --no-sync python -c "import nvdaRust; print('OK', nvdaRust.crashdump.writeCrashDump.__name__)"
```

Expected: `OK writeCrashDump`. Confirms uv-launched Python still finds nvdaRust in site-packages.

* [ ] **Step 5: No commit**

This task is verification only.

---

## Task 4: Update runnvda.bat to use --no-sync

**Files:**

* Modify: `runnvda.bat`

The current `runnvda.bat`:

```batch
@echo off
set hereOrig=%~dp0
set here=%hereOrig%
if #%hereOrig:~-1%# == #\# set here=%hereOrig:~0,-1%
set sourceDirPath=%here%\source

start uvw run --gui-script --directory "%sourceDirPath%" nvda.pyw %*
```

The `uvw run` triggers an implicit `uv sync` on every NVDA start. With nvdaRust no longer in the uv workspace (Task 2), the sync is harmless for nvdaRust — but it still costs ~5s and would re-resolve other workspace members unnecessarily. `--no-sync` skips that. Devs wanting to refresh other deps run `uv sync` manually.

* [ ] **Step 1: Add `--no-sync` to the `uvw run` invocation**

Replace the last line with:

```batch
start uvw run --no-sync --gui-script --directory "%sourceDirPath%" nvda.pyw %*
```

* [ ] **Step 2: Verify**

Quick sanity-check by starting NVDA from the source build:

* Close any running NVDA.
* Run `runnvda.bat` from a Windows shell.
* Confirm NVDA starts (announces "NVDA started" or similar).
* Confirm startup is faster (no 5-7s sync delay before NVDA's own init).

If NVDA fails to start because some other Python dep is missing from the venv, run `uv sync` once explicitly, then retry. That should be a one-time setup; the steady-state dev workflow is `scons source && runnvda.bat`.

* [ ] **Step 3: Commit**

```bash
git add runnvda.bat
git commit -m "runnvda: skip uv sync on launch (SCons owns nvdaRust now)"
```

---

## Task 5: End-to-end verification

**Files:** none modified — this is a verification gate.

* [ ] **Step 1: Verify the dev workflow**

Make a trivial change to a Rust source file (any `rust/nvda_*/src/lib.rs` — e.g., add a comment):

```bash
echo "// dev-workflow-verification touch" >> rust/nvda_ole/src/lib.rs
```

Run `scons source --all-cores`. Expected: SCons triggers maturin develop (you'll see cargo compile output if anything changed), the `.pyd` mtime in `.venv` updates.

Revert the test change:

```bash
git checkout rust/nvda_ole/src/lib.rs
```

* [ ] **Step 2: Verify NVDA picks up the change**

Run `runnvda.bat`. Open NVDA's Python console. Run:

```python
import nvdaRust, os
pyd = os.path.join(nvdaRust.__path__[0], 'nvdaRust.cp313-win_amd64.pyd')
print('mtime:', os.path.getmtime(pyd))
```

Expected: mtime matches the Step 1 build (within seconds).

Also verify the pyo3-log routing still works:

```python
try: nvdaRust.ole.getOleClipboardText(0)
except OSError: pass
```

Expected: a `WARNING - nvda_ole.None` entry in NVDA's log (same as the verification we just landed in the previous PR).

* [ ] **Step 3: Verify the production workflow**

Build the full installer:

```
./scons.bat launcher --all-cores release=1 2>&1 | tail -5
```

Expected: `scons: done building targets.` This invokes the full build chain including a release-profile cargo build of nvda_python.

Verify the resulting installer exists at `output/<some>.exe`. Optionally inspect its bundled `nvdaRust.pyd` size — should be ~500-700 KB (release profile, much smaller than the dev-build 1.3 MB).

* [ ] **Step 4: Confirm clean working tree**

```
git status -s
```

Expected: only the unstaged submodule entries we already know about.

* [ ] **Step 5: No commit**

Verification gate only.

---

## Task 6: Update documentation in the input-hooks plan

The input-hooks plan (`docs/plans/2026-05-02-rust-input-hooks-derisk.md`) was written under the assumption that nvda_python continues to be uv-managed. With this plan landing, the workspace pattern changes: BOTH nvda_python (cdylib for PyO3) and nvda_input_hooks (staticlib for nvdaHelperRemote.dll) are now SCons-built. Update the input-hooks plan's "Notes for future ports" section to reflect this.

**Files:**

* Modify: `docs/plans/2026-05-02-rust-input-hooks-derisk.md`

* [ ] **Step 1: Append a note to the "Notes for future ports" section**

Find the heading `## Notes for future ports` near the bottom of `docs/plans/2026-05-02-rust-input-hooks-derisk.md`. Append at the end of that section:

```markdown
### Update: SCons now owns the cargo workflow for both crate types

The plan above describes the SCons cargo block in `nvdaHelper/remote/sconscript` for the staticlib (`nvda_input_hooks`). Subsequently, `nvda_python` (the PyO3 cdylib that becomes `nvdaRust.pyd`) was also moved from the uv workspace to a SCons-built target — see `docs/plans/2026-05-03-rust-scons-integration.md` for the rationale (uv wheel-cache trap, no production fail-safe).

Future Rust ports follow this pattern:

* **PyO3 cdylib (loaded into NVDA's main process):** add to `nvda_python` as a submodule, no separate SCons build needed (already covered by the nvda_python build target in `sconstruct`).
* **Staticlib for an injected DLL (e.g. `nvdaHelperRemote.dll`):** add an entry to the existing cargo block in that DLL's sconscript (currently only `nvdaHelper/remote/sconscript`), gating to x86_64 only until multi-arch cargo is solved.

Both share the SCons `release` BoolVariable for profile selection.
```

* [ ] **Step 2: Commit**

```bash
git add docs/plans/2026-05-02-rust-input-hooks-derisk.md
git commit -m "docs: note SCons-driven nvdaRust integration in input-hooks plan"
```

---

## Task 7: Final sweep + push

* [ ] **Step 1: Confirm clean tree**

```
git status -s
```

Expected: only the submodule entries we already know about.

* [ ] **Step 2: Run cargo tests**

```
cd rust && cargo test --workspace 2>&1 | grep "test result" | head -10
```

Expected: all 50+ existing tests still pass.

* [ ] **Step 3: Run Python unit suite**

```
./rununittests.bat 2>&1 | tail -5
```

Expected: `Ran 1164+ tests in <T>s, OK`.

* [ ] **Step 4: Show commit log**

```
git log --oneline origin/master..HEAD | head -10
```

Expected: 4 new commits on top of the pyo3-log work — sconstruct nvdaRust builder, pyproject removal, runnvda --no-sync, input-hooks plan note.

* [ ] **Step 5: Push**

Per project convention, do NOT open a PR. Push and let the user eyeball the diff before opening anything.

```bash
git push origin HEAD
```

---

## Out of scope

* **Multi-arch nvdaRust.pyd.** Each Python interpreter arch needs its own `.pyd`; current build host arch (x86_64) is sufficient for dev. Per-arch build is a packaging concern for the final installer.
* **Refactoring `nvda_input_hooks` SCons cargo block.** Already SCons-built since commit `46b6ddd93`; no change needed here.
* **`[tool.maturin]` profile config.** Profile is selected via the `--release` flag passed to `maturin develop`, not via `pyproject.toml`. Keeps SCons as the single source of truth for build-mode selection.
* **Removing the cargo workspace.** The cargo workspace at `rust/Cargo.toml` is independent of the uv workspace and stays unchanged.
* **`#![no_std]` for the input-hooks crate.** Still a valid follow-up for reducing per-injected-process Rust footprint, but unrelated to this plan.
