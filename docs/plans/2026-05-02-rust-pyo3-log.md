# Rust pyo3-log integration — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the `LOG_*` diagnostics that were lost when `oleUtils.cpp` and `screenCurtain.cpp` were ported to Rust. Routes Rust `log` macros through `pyo3-log` to NVDA's Python `logging` module, so failures land in NVDA's log file the same way they did in the C++ implementations.

**Architecture:** Add the standard Rust `log` crate as a dependency to the in-process Rust crates that need diagnostics (`nvda_ole`, `nvda_screen_curtain`). Add `pyo3-log` to `nvda_python` only and call `pyo3_log::init()` once during the PyO3 module init. The Rust crates use `log::warn!`/`log::error!`/`log::debug!` macros at the same call sites the C++ originals used `LOG_DEBUGWARNING`/`LOG_ERROR`/`LOG_DEBUG`. `pyo3-log` auto-creates Python loggers named after each Rust crate (e.g. `nvda_ole`, `nvda_screen_curtain`), which inherit through Python's logging hierarchy to NVDA's root handler. No call-site changes in Python.

**Tech Stack:** Rust 2021, `log` 0.4 (the standard Rust logging facade), `pyo3-log` 0.12 (or current — verify on add), no other new deps. The `log` crate is `no_std`-compatible (matters for the future `nvda_input_hooks` clean-up) so this also keeps the door open to using the same macros in injected-DLL Rust code with a different backend.

---

## Why now?

Two recent code reviews flagged this as a gap:

1. **`nvda_ole` review** (commit `5b14ad59d` lineage) noted that `nvda_wasapi`'s pattern of returning `windows::core::Error` so the PyO3 wrapper can format `e.message()` was a richer model than `nvda_ole`'s bare HRESULT-int return. Practical impact today is near-zero because both call sites in `edit.py` `pass` on `OSError`. The richer story isn't visible because we have nowhere to log the cause from inside Rust.

2. **`nvda_screen_curtain` review** (commit `e26eb8b51` lineage) flagged "loss of granular failure logging" as the main observability concern: the C++ original had 10 `LOG_ERROR` calls at distinct GDI/GDI+ failure points; the Rust port silently returns `false`. A real GDI failure in the field is now indistinguishable from a non-black screen.

This plan closes both gaps with a single shared mechanism: the `log` crate. No bespoke wrappers; future Rust crates just `use log::warn` and the diagnostics flow.

It also locks in `log` as the diagnostic abstraction across the Rust workspace. When we eventually port `log.cpp` for the `remote/` injected-DLL side, the in-process and injected-DLL Rust code will share the same call-site macros — only the logger backend differs (`pyo3-log` for in-process, custom MIDL-backed logger for injected).

---

## Scope

**In scope:**

* Adding `log` + `pyo3-log` dependencies and wiring init.
* Restoring the C++ `LOG_*` call sites in `nvda_ole` and `nvda_screen_curtain` as Rust `log::*` macro calls.
* Verifying via Python integration test that messages flow through `pyo3-log` to Python's `logging`.

**Out of scope:**

* Adding logging to `nvda_text`, `nvda_tones`, `nvda_crashdump` (their C++ originals had no `LOG_*` calls — there's nothing to restore).
* Adding logging to `nvda_wasapi` (already-on-branch port; if it has any silent failure paths, that's a separate audit).
* Anything in `nvdaHelper/remote/` (injected DLLs have no Python; `pyo3-log` is irrelevant there).
* Restructuring `nvda_ole` to return `windows::core::Error` instead of `i32` HRESULT (orthogonal improvement; can be a separate plan once we decide whether to unify error types across crates).

---

## File Structure

**Modify:**

* `rust/nvda_python/Cargo.toml` — add `pyo3-log = "0.12"` and `log = "0.4"` dependencies.
* `rust/nvda_python/src/lib.rs` — add `#[pymodule_init]` function that calls `pyo3_log::init()`.
* `rust/nvda_ole/Cargo.toml` — add `log = "0.4"` dependency.
* `rust/nvda_ole/src/lib.rs` — add `log::warn!` calls at the 8 documented C++ failure sites (4 in `get_clipboard_text`, 4 in `get_user_type`).
* `rust/nvda_screen_curtain/Cargo.toml` — add `log = "0.4"` dependency.
* `rust/nvda_screen_curtain/src/lib.rs` — add `log::error!` calls at the 10 documented C++ failure sites + 1 `log::debug!` for the histogram trace.
* `tests/manual/rust/oleIntegration.py` — extend with a Python logging handler that captures records, verify the null-IUnknown case produces a `WARNING` record.

**No new files. No deletions.**

---

## Working assumptions

1. **NVDA's Python logger picks up child loggers automatically.** NVDA's `logHandler.log = logging.getLogger("nvda")` is configured with handlers. Python's logging hierarchy means the loggers `pyo3-log` creates (named after Rust crates: `nvda_ole`, `nvda_screen_curtain`) propagate up to the root logger, which inherits NVDA's handlers. If the messages don't flow to NVDA's log file with the default `pyo3_log::init()`, the fallback is `Logger::new(py, Caching::LoggersAndLevels).filter(LevelFilter::Debug).install()` with explicit configuration; document the choice in the commit message if needed.
2. **`pyo3-log` 0.12 is current.** Check `cargo search pyo3-log` and bump if there's a newer version. The crate is small and stable; major-version-only API breaks.
3. **No CRT or feature-gate surprises.** `pyo3-log` is pure Rust + PyO3; the existing PyO3 0.28 dep covers it. `log` 0.4 is the Rust ecosystem standard, no Win32 features.
4. **Python's logging works at the test process's default level.** When running `uv run python tests/manual/rust/oleIntegration.py`, Python's logging module is configured to its defaults (no `basicConfig()` called), which means handlers must be installed by the test if it wants to capture records. The test installs its own handler.

---

## Task 1: Add `pyo3-log` to `nvda_python` and wire init

**Files:**

* Modify: `rust/nvda_python/Cargo.toml`
* Modify: `rust/nvda_python/src/lib.rs`

* [ ] **Step 1: Verify the current pyo3-log version**

Run:

```
cargo search pyo3-log 2>&1 | head -3
```

Expected: a line like `pyo3-log = "0.12.4"` (or whatever's current). Use that exact version in the Cargo.toml change below — bump from `0.12` to whatever the latest minor-or-patch is.

* [ ] **Step 2: Add deps to `rust/nvda_python/Cargo.toml`**

Find the existing `[dependencies]` block. After `pyo3 = { version = "0.28", features = ["extension-module"] }`, add:

```toml
log = "0.4"
pyo3-log = "0.12"
```

(Adjust the `pyo3-log` version to match what `cargo search` reported.)

* [ ] **Step 3: Add the pymodule init function**

In `rust/nvda_python/src/lib.rs`, find the outer `#[pymodule] mod nvda_rust { ... }` block (currently around lines 119–135). Add a `#[pymodule_init]` function inside it, BEFORE the existing `#[pymodule_export]` lines:

```rust
#[pymodule]
#[pyo3(name = "nvdaRust")]
mod nvda_rust {
    #[pymodule_init]
    fn init(_m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        // Forward Rust `log` macros to Python's `logging` module. NVDA's root
        // logger handlers pick up child loggers (named after the Rust crate)
        // automatically via Python's logging hierarchy.
        pyo3_log::init();
        Ok(())
    }

    #[pymodule_export]
    use super::crashdump_mod;
    #[pymodule_export]
    use super::ole_mod;
    #[pymodule_export]
    use super::screen_curtain_mod;
    #[pymodule_export]
    use super::text_mod;
    #[pymodule_export]
    use super::tones_mod;
    #[pymodule_export]
    use super::wasapi_mod;
}
```

(Preserve the existing `#[pymodule_export]` lines — add the `init` function ahead of them; don't replace them.)

* [ ] **Step 4: Build the wheel and confirm import**

Run:

```
cd rust/nvda_python && uvx maturin develop 2>&1 | tail -3
```

Expected: `Installed nvdaRust-0.1.0`.

Then sync (`uv` reinstalls the editable wheel):

```
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator && uv sync --reinstall-package nvdaRust 2>&1 | tail -2
```

Smoke test:

```
uv run python -c "import logging; logging.basicConfig(level=logging.DEBUG); import nvdaRust; print('OK', nvdaRust.crashdump.writeCrashDump.__name__)"
```

Expected: prints `OK writeCrashDump` with no errors. The `pyo3_log::init()` call returns Ok even with no log records emitted yet — we're just verifying the wheel still imports.

* [ ] **Step 5: Commit**

```bash
git add rust/nvda_python/Cargo.toml rust/nvda_python/src/lib.rs rust/Cargo.lock
git commit -m "nvda_python: route Rust log macros to Python logging via pyo3-log"
```

---

## Task 2: Restore `LOG_DEBUGWARNING` calls in `nvda_ole`

The C++ `oleUtils.cpp` (deleted in commit `4903f9521`) had 8 `LOG_DEBUGWARNING` and 1 `LOG_ERROR` call at distinct failure points across `get_clipboard_text` and `get_user_type`. We restore the 8 DEBUGWARNING ones (the LOG_ERROR site, "Failed to get IMalloc interface", was about an `IMalloc::Free` path that the Rust port replaced with `CoTaskMemFree` — that error mode is gone).

The C++ used `LOG_DEBUGWARNING` (custom NVDA level between DEBUG and WARNING). The closest standard `log` level is `Warn` — visible in NVDA's default config without being noisy. Use `log::warn!` for these.

**Files:**

* Modify: `rust/nvda_ole/Cargo.toml`
* Modify: `rust/nvda_ole/src/lib.rs`

* [ ] **Step 1: Add `log` dep to `nvda_ole/Cargo.toml`**

Find the existing `[dependencies]` block (or `[dependencies.windows-core]` block — they're together). Add at the bottom of the dependency declarations, before any `[dev-dependencies]`:

```toml
[dependencies]
log = "0.4"
```

If `[dependencies]` doesn't exist yet (the file currently uses `[dependencies.windows-core]` and `[dependencies.windows]` table-style), add a plain `[dependencies]` block before them:

```toml
[dependencies]
log = "0.4"

[dependencies.windows-core]
version = "0.58"
...
```

Verify with `cargo check -p nvda_ole 2>&1 | tail -3` — expect `Finished ...`.

* [ ] **Step 2: Add `log::warn!` calls at the 4 sites in `get_clipboard_text`**

In `rust/nvda_ole/src/lib.rs`, find `get_clipboard_text`. The current implementation:

```rust
pub fn get_clipboard_text(unknown_addr: usize) -> OleResult {
    let unknown = unsafe { borrow_iunknown(unknown_addr) }.ok_or(E_INVALIDARG.0)?;
    let data_object: IDataObject = unknown.cast().map_err(|_| E_NOINTERFACE.0)?;

    let format = FORMATETC { ... };

    let medium: STGMEDIUM = unsafe { data_object.GetData(&format) }.map_err(|e| e.code().0)?;

    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let hglobal = unsafe { medium.u.hGlobal };
    if hglobal.is_invalid() {
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let text = unsafe {
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            // Deliberate divergence from C++: oleUtils.cpp returned S_OK with
            // an empty BSTR when GlobalLock failed. We surface the failure as
            // an HRESULT so the Python caller can fall through cleanly.
            ReleaseStgMedium(&mut { medium });
            return Err(E_FAIL.0);
        }
        // ... read text ...
    };
    // ...
    Ok(text)
}
```

Replace it with the version below. Each `log::warn!` matches a `LOG_DEBUGWARNING` from `oleUtils.cpp:21,26,33,37`:

```rust
pub fn get_clipboard_text(unknown_addr: usize) -> OleResult {
    let unknown = match unsafe { borrow_iunknown(unknown_addr) } {
        Some(u) => u,
        None => {
            log::warn!("getOleClipboardText: pUnknown is null.");
            return Err(E_INVALIDARG.0);
        }
    };
    let data_object: IDataObject = unknown.cast().map_err(|_| {
        log::warn!("getOleClipboardText: could not get IDataObject interface from pUnknown");
        E_NOINTERFACE.0
    })?;

    let format = FORMATETC {
        cfFormat: CF_UNICODETEXT.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };

    let medium: STGMEDIUM = unsafe { data_object.GetData(&format) }.map_err(|e| {
        log::warn!("getOleClipboardText: IDataObject::GetData failed with HRESULT 0x{:08x}", e.code().0 as u32);
        e.code().0
    })?;

    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        log::warn!("getOleClipboardText: got back invalid medium (tymed = {})", medium.tymed);
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let hglobal = unsafe { medium.u.hGlobal };
    if hglobal.is_invalid() {
        log::warn!("getOleClipboardText: medium.hGlobal is invalid");
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let text = unsafe {
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            // Deliberate divergence from C++: oleUtils.cpp returned S_OK with
            // an empty BSTR when GlobalLock failed. We surface the failure as
            // an HRESULT so the Python caller can fall through cleanly.
            log::warn!("getOleClipboardText: GlobalLock returned null");
            ReleaseStgMedium(&mut { medium });
            return Err(E_FAIL.0);
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let s = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(hglobal);
        s
    };

    unsafe { ReleaseStgMedium(&mut { medium }) };
    Ok(text)
}
```

(Two new sites beyond the C++ original: `medium.hGlobal is invalid` and `GlobalLock returned null`. The C++ silently returned without these. They're failure paths the Rust port surfaces; logging them is consistent with the spirit of the rest.)

* [ ] **Step 3: Add `log::warn!` calls at the 4 sites in `get_user_type`**

Replace `get_user_type` with:

```rust
pub fn get_user_type(unknown_addr: usize, flags: u32) -> OleResult {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Ole::USERCLASSTYPE;

    let unknown = match unsafe { borrow_iunknown(unknown_addr) } {
        Some(u) => u,
        None => {
            log::warn!("getOleUserType: pUnknown is null.");
            return Err(E_INVALIDARG.0);
        }
    };
    let ole_object: IOleObject = unknown.cast().map_err(|_| {
        log::warn!("getOleUserType: could not get IOleObject interface from pUnknown");
        E_NOINTERFACE.0
    })?;

    let ole_str = unsafe { ole_object.GetUserType(USERCLASSTYPE(flags as i32)) }.map_err(|e| {
        log::warn!(
            "getOleUserType: IOleObject::GetUserType failed with HRESULT 0x{:08x} for flags {}",
            e.code().0 as u32,
            flags,
        );
        e.code().0
    })?;
    if ole_str.is_null() {
        log::warn!("getOleUserType: IOleObject::GetUserType returned null string for flags {}", flags);
        return Err(E_FAIL.0);
    }

    let result = unsafe {
        let mut len = 0usize;
        while *ole_str.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ole_str.0, len);
        let s = String::from_utf16_lossy(slice);
        CoTaskMemFree(Some(ole_str.0 as *const _));
        s
    };

    Ok(result)
}
```

If the existing `use` lines for `USERCLASSTYPE` and `CoTaskMemFree` are at function scope, leave them as you found them (the inline `use` shown above matches the current state of `lib.rs` at commit `5b14ad59d`).

* [ ] **Step 4: Verify it compiles**

```
cd rust && cargo check -p nvda_ole 2>&1 | tail -5
```

Expected: `Finished dev profile [...] in <N>s`. No warnings.

* [ ] **Step 5: Commit**

```bash
git add rust/nvda_ole/Cargo.toml rust/nvda_ole/src/lib.rs rust/Cargo.lock
git commit -m "nvda_ole: log diagnostics at COM failure points (matches C++ LOG_DEBUGWARNING)"
```

---

## Task 3: Restore `LOG_ERROR` calls in `nvda_screen_curtain`

The C++ `screenCurtain.cpp` (deleted in commit `4903f9521`) had 10 `LOG_ERROR` calls at GDI/GDI+ failure points and 1 `LOG_DEBUG` for the histogram trace. Map them as:

* All 10 `LOG_ERROR` → `log::error!`
* The histogram trace → `log::debug!` (Rust's `log::debug!` ≈ Python `DEBUG`, which matches the C++ `LOG_DEBUG`).

**Files:**

* Modify: `rust/nvda_screen_curtain/Cargo.toml`
* Modify: `rust/nvda_screen_curtain/src/lib.rs`

* [ ] **Step 1: Add `log` dep to `nvda_screen_curtain/Cargo.toml`**

Add to `[dependencies]` (creating the block if absent, same pattern as Task 2 Step 1):

```toml
[dependencies]
log = "0.4"
```

Verify with `cargo check -p nvda_screen_curtain 2>&1 | tail -3`.

* [ ] **Step 2: Add `log::error!` at each failure path in `is_screen_fully_black`**

Open `rust/nvda_screen_curtain/src/lib.rs` and find the function. The current shape has multiple `return false` paths; add a `log::error!` immediately before each `return false`. Walk through every `return false` site (the previous code review traced 12 of them, with 4 RAII guards covering exit paths). For each one, add a matching log line per this table (line numbers are approximate — search for the failure description in the existing comments):

| Failure | Log call |
| --- | --- |
| `desktop_wnd.is_invalid()` | `log::error!("isScreenFullyBlack: failed to get handle for desktop window");` |
| `GetDC` returned invalid HDC | `log::error!("isScreenFullyBlack: failed to get device context for desktop");` |
| `CreateCompatibleDC` failed | `log::error!("isScreenFullyBlack: failed to create compatible device context");` |
| `CreateCompatibleBitmap` returned null | `log::error!("isScreenFullyBlack: failed to create compatible bitmap");` |
| `SelectObject` returned null | `log::error!("isScreenFullyBlack: failed to select capture bitmap into capture device context");` |
| `BitBlt` returned false | `log::error!("isScreenFullyBlack: BitBlt failed (GetLastError = {})", unsafe { windows::Win32::Foundation::GetLastError().0 });` |
| `GetObjectW` returned 0 | `log::error!("isScreenFullyBlack: failed to get bitmap metadata");` |
| `GdipCreateBitmapFromGdiDib` failed (Status != Ok or null bitmap) | `log::error!("isScreenFullyBlack: GdipCreateBitmapFromGdiDib failed (status = {:?})", status);` |
| `GdipBitmapGetHistogramSize` returned non-Ok | `log::error!("isScreenFullyBlack: GdipBitmapGetHistogramSize failed (status = {:?})", status);` |
| `GdipBitmapGetHistogram` returned non-Ok | `log::error!("isScreenFullyBlack: GdipBitmapGetHistogram failed (status = {:?})", status);` |
| `GetDIBits` returned 0 | `log::error!("isScreenFullyBlack: GetDIBits failed (lines_copied = 0)");` |
| Vec allocation failure (`Vec::try_reserve` or similar — only if the code uses fallible allocation; otherwise this path doesn't exist in Rust because `vec![...; n]` aborts on OOM, mirroring what the C++ `bad_alloc` catch block did) | If applicable: `log::error!("isScreenFullyBlack: failed to allocate buffers");` |

If any failure path doesn't already exist in the current Rust code (e.g., the `bad_alloc` equivalent), don't invent it — just log the paths that exist.

**Engineer note:** Use `windows::Win32::Foundation::GetLastError` with the `Win32_Foundation` feature already in this crate's Cargo.toml. If the BitBlt failure path doesn't currently call `GetLastError`, you can either add the import + call (matches the C++ original which logged it), or omit the GetLastError part and log just the failure description. Match the C++ original's behavior — it included `GetLastError`.

* [ ] **Step 3: Add `log::debug!` for the histogram trace**

The C++ original wrote a line like `"Histogram of virtual screen: (R0, G0, B0) (R1, G1, B1) ..."`. Add an equivalent in Rust after `GdipBitmapGetHistogram` succeeds, just before the all-channels-zero check:

```rust
if log::log_enabled!(log::Level::Debug) {
    let mut summary = String::with_capacity(histogram_size as usize * 16);
    summary.push_str("Histogram of virtual screen:");
    for i in 0..histogram_size as usize {
        summary.push_str(&format!(" ({}, {}, {})", hist_r[i], hist_g[i], hist_b[i]));
    }
    log::debug!("{}", summary);
}
```

The `log::log_enabled!` gate avoids paying the histogram-formatting cost when DEBUG logging is off (which is most of the time — this trace runs every time screen-curtain re-checks the screen state).

* [ ] **Step 4: Verify it compiles**

```
cd rust && cargo check -p nvda_screen_curtain 2>&1 | tail -5
```

Expected: clean build.

* [ ] **Step 5: Commit**

```bash
git add rust/nvda_screen_curtain/Cargo.toml rust/nvda_screen_curtain/src/lib.rs rust/Cargo.lock
git commit -m "nvda_screen_curtain: log GDI/GDI+ failures (matches C++ LOG_ERROR)"
```

---

## Task 4: Verify log routing through pyo3-log

The integration test extends the existing manual OLE test to install a Python `logging` handler that captures records, then verifies the null-IUnknown case produces a `WARNING` record routed via `pyo3-log` from Rust.

**Files:**

* Modify: `tests/manual/rust/oleIntegration.py`

* [ ] **Step 1: Read the existing test**

Run `cat tests/manual/rust/oleIntegration.py | head -30` to confirm the file structure. The current `main()` runs `runCase` for clipboard cases, then `runNullIUnknownCase` for the null-IUnknown error path, then `runUserTypeCases` for the OLE class cases.

* [ ] **Step 2: Add a logging-capture helper near the top of the file (after imports)**

Insert this block AFTER the `import` lines at the top of the file (they're at lines 41–45 currently: `ctypes`, `sys`, `time`, `nvdaRust`):

```python
import logging


class _RecordCollector(logging.Handler):
	"""Captures log records emitted via Python's logging module so the manual
	test can verify Rust-emitted log calls flow through pyo3-log."""

	def __init__(self):
		super().__init__()
		self.records: list[logging.LogRecord] = []

	def emit(self, record: logging.LogRecord) -> None:
		self.records.append(record)


def _installLogCapture(level: int = logging.DEBUG) -> _RecordCollector:
	"""Install a record-capturing handler at the given level. Returns the handler."""
	collector = _RecordCollector()
	collector.setLevel(level)
	root = logging.getLogger()
	root.addHandler(collector)
	# Default root level is WARNING; lower it so DEBUG-level Rust messages flow.
	if root.level > level or root.level == logging.NOTSET:
		root.setLevel(level)
	return collector
```

* [ ] **Step 3: Modify the existing `runNullIUnknownCase` to also verify a log record was emitted**

Replace the current `runNullIUnknownCase` function with:

```python
def runNullIUnknownCase(collector: _RecordCollector) -> bool:
	beforeCount = len(collector.records)
	try:
		nvdaRust.ole.getOleClipboardText(0)
	except OSError as e:
		# Verify the Rust side emitted a WARNING-level log record.
		newRecords = collector.records[beforeCount:]
		oleRecords = [r for r in newRecords if r.levelno == logging.WARNING and "pUnknown is null" in r.getMessage()]
		if oleRecords:
			print(f"  PASS  null IUnknown raises OSError ({e}) and emits WARNING log record")
			return True
		print(
			f"  FAIL  null IUnknown raised OSError but no matching WARNING record found. "
			f"Captured {len(newRecords)} new record(s); levels: {[r.levelno for r in newRecords]}",
		)
		return False
	print("  FAIL  null IUnknown should have raised OSError")
	return False
```

* [ ] **Step 4: Wire the collector into `main()`**

In `main()`, install the collector before the test cases run and pass it to `runNullIUnknownCase`. Find the existing `main()` function and update its body. The current shape is roughly:

```python
def main() -> int:
	ole32.OleInitialize(None)
	saved = getClipboardUnicode()
	failures = 0
	try:
		print("getOleClipboardText:")
		cases = [...]
		for name, text in cases:
			if not runCase(name, text):
				failures += 1
		if not runNullIUnknownCase():
			failures += 1
		# ... user-type cases ...
	finally:
		# ... restore clipboard, OleUninitialize ...
	# ... summary print ...
```

Replace the body with:

```python
def main() -> int:
	ole32.OleInitialize(None)
	collector = _installLogCapture(level=logging.DEBUG)
	saved = getClipboardUnicode()
	failures = 0
	try:
		print("getOleClipboardText:")
		cases = [
			("ASCII", "Hello NVDA"),
			("CJK", "日本語テスト"),
			(
				"ZWJ emoji",
				"\U0001f468‍\U0001f469‍\U0001f467‍\U0001f466",
			),
			("empty", ""),
		]
		for name, text in cases:
			if not runCase(name, text):
				failures += 1
		if not runNullIUnknownCase(collector):
			failures += 1

		print()
		print("getOleUserType:")
		userTypeFailures, _ = runUserTypeCases()
		failures += userTypeFailures
	finally:
		if saved is not None:
			try:
				setClipboardUnicode(saved)
			except OSError:
				pass
		ole32.OleUninitialize()
	print()
	print(f"Captured {len(collector.records)} total log record(s) during run.")
	print(f"{'PASS' if failures == 0 else 'FAIL'} ({failures} failure(s))")
	return 0 if failures == 0 else 1
```

(The only changes vs. the existing `main()`: the `collector = _installLogCapture(...)` line, the `runNullIUnknownCase(collector)` argument, and the `Captured ... record(s)` summary print before the final PASS/FAIL.)

* [ ] **Step 5: Run the test and verify**

```
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator && uv run python tests/manual/rust/oleIntegration.py 2>&1 | tail -15
```

Expected output:

```
getOleClipboardText:
  PASS  ASCII
  PASS  CJK
  PASS  ZWJ emoji
  PASS  empty
  PASS  null IUnknown raises OSError (HRESULT 0x80070057) and emits WARNING log record

getOleUserType:
  PASS  Word.Document FULL: 'Microsoft Word Document'
  PASS  Word.Document SHORT: 'Document'
  PASS  Word.Document APPNAME: 'Microsoft Word Document'
  PASS  getOleUserType null IUnknown raises OSError (HRESULT 0x80070057)

Captured 1 total log record(s) during run.
PASS (0 failure(s))
```

The `Captured 1 total log record(s)` line confirms `pyo3-log` is forwarding the Rust `log::warn!` call into Python's logging system. If the count is 0, `pyo3_log::init()` isn't running (revisit Task 1) or the log level isn't propagating (try `_installLogCapture(level=logging.DEBUG)` instead — already the default).

If the count is >1, the user-type test path is also triggering log records (it does pass null to `getOleUserType` later). That's expected and harmless — adjust the assertion message above if so.

* [ ] **Step 6: Commit**

```bash
git add tests/manual/rust/oleIntegration.py
git commit -m "oleIntegration: verify Rust log macros flow through pyo3-log"
```

---

## Task 5: Manual NVDA verification

A round-trip from "trigger an OLE failure in NVDA" to "see the message in NVDA's log" — proves end-to-end that the production NVDA log file picks up Rust diagnostics.

* [ ] **Step 1: Build NVDA from this source tree**

```
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator && ./scons.bat source --all-cores 2>&1 | tail -5
```

Expected: `scons: done building targets.`

* [ ] **Step 2: Set NVDA's log level to DEBUG and clear the log**

In NVDA's general settings (`NVDA+control+g` → General), set log level to "Debug". Save and apply.

Locate NVDA's log file (typically `%TEMP%\nvda.log` or whatever path the user's `globalVars.appArgs.logFileName` points to). Note the current size or trim it.

* [ ] **Step 3: Trigger a known Rust diagnostic**

The cleanest way to trigger a Rust-emitted log message in production NVDA: enable screen curtain (`NVDA+ctrl+escape`). The first call into `is_screen_fully_black` succeeds (screen really is black) so no `log::error!` fires. To get a `log::debug!` to fire, leave NVDA running with screen curtain on for a few seconds while debug logging is enabled — the periodic histogram trace will emit.

Alternatively, use the OLE clipboard path: open Outlook (or any editor with embedded OLE), navigate to a malformed embedded object that fails the IDataObject path. This is harder to reliably trigger.

**Easiest path:** add a temporary diagnostic line at NVDA startup that calls `nvdaRust.ole.getOleClipboardText(0)` to deliberately trigger the null-IUnknown WARN. Catch the OSError. Look for the message in the log.

```python
# Temporarily add to source/core.py or wherever post-init code runs:
import nvdaRust
try:
    nvdaRust.ole.getOleClipboardText(0)
except OSError:
    pass
```

* [ ] **Step 4: Verify the log record landed**

Open NVDA's log file. Search for "pUnknown is null" or "getOleClipboardText". Expected: a line like:

```
WARNING - nvda_ole (XX:XX:XX.XXX):
getOleClipboardText: pUnknown is null.
```

The exact format depends on NVDA's log formatter. The key thing: a line containing the Rust message text appears at WARNING level.

* [ ] **Step 5: Remove the temporary diagnostic line**

Undo the change to `source/core.py` (or wherever it was added). Don't commit it.

* [ ] **Step 6: No commit**

This is a verification gate. If the log record didn't land, the integration is broken at the NVDA-level (logger config, root-vs-named-logger mismatch, etc.). Investigate before proceeding.

---

## Task 6: Final sweep + push

* [ ] **Step 1: Confirm clean working tree**

```
git status -s
```

Expected: only the unstaged submodule entries we already know about.

* [ ] **Step 2: Run cargo tests**

```
cd rust && cargo test --workspace 2>&1 | grep "test result" | head -10
```

Expected: all 50+ existing tests still pass. `nvda_input_hooks` test target is disabled per its `Cargo.toml`.

* [ ] **Step 3: Run Python unit tests**

```
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator && ./rununittests.bat 2>&1 | tail -5
```

Expected: all 1164+ tests pass. None of this work touches Python tests directly.

* [ ] **Step 4: Re-run the extended OLE integration test**

```
uv run python tests/manual/rust/oleIntegration.py 2>&1 | tail -15
```

Expected: same output as Task 4 Step 5.

* [ ] **Step 5: Show the commit log for review**

```
git log --oneline origin/master..HEAD
```

Expected: 4 new commits on top of the input-hooks de-risk work — pyo3-log wiring, nvda_ole logs, nvda_screen_curtain logs, oleIntegration capture.

* [ ] **Step 6: Push**

Per project convention, do NOT open a PR. Push and let the user eyeball the diff before opening anything.

```
git push origin HEAD
```

---

## Out of scope

* **Adding logging to `nvda_wasapi`** — separate audit; if its silent-failure paths are worth surfacing, file a follow-up.
* **Restructuring `nvda_ole` to return `windows::core::Error`** — was the original review suggestion that motivated logging. With `log::warn!` in place, the HRESULT is now visible in logs even though the API still returns `i32`. The `windows::core::Error` migration is a separate cleanup.
* **A Rust logger backend for `nvdaHelper/remote/`** — the injected DLLs need a different backend (no Python). When that's tackled, the in-process Rust crates already use the `log` macro abstraction — only the backend changes.
* **Custom NVDA log levels** — NVDA's `LOG_DEBUGWARNING` doesn't have an exact `log` crate equivalent (the standard levels are Error/Warn/Info/Debug/Trace). We map DEBUGWARNING → Warn. If finer granularity is wanted, a custom `pyo3_log::Logger` configuration with NVDA-specific level mapping can be added later — not needed for the diagnostic-restoration goal.
