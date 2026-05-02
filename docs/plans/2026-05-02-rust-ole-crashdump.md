# Rust OLE + crash dump port — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `nvdaHelper/local/oleUtils.cpp` and `nvdaHelper/local/crashDump.cpp` to two new Rust crates (`nvda_ole`, `nvda_crashdump`), expose them through the existing `nvdaRust` PyO3 module, switch the three Python call sites, and remove the C++ originals.

**Architecture:** Two small standalone crates, each one file. `nvda_ole` wraps `IDataObject::GetData(CF_UNICODETEXT)` and `IOleObject::GetUserType` via `windows-rs` 0.58, accepting raw COM pointers as `usize` from Python. `nvda_crashdump` calls `MiniDumpWriteDump` via the same `windows` crate. PyO3 bindings live in the existing `rust/nvda_python` crate as `nvdaRust.ole` and `nvdaRust.crashdump` submodules — same pattern as `nvdaRust.text` (commit `efb0e2b55`) and `nvdaRust.tones`.

**Tech Stack:** Rust 2021, PyO3 0.28, `windows` 0.58 with features `Win32_System_Com`, `Win32_System_Ole`, `Win32_System_Com_StructuredStorage`, `Win32_System_Memory`, `Win32_Graphics_Gdi` (for OLE — the StructuredStorage and Gdi features are required by `STGMEDIUM`/`ReleaseStgMedium`/HGLOBAL gating), `Win32_System_Diagnostics_Debug`, `Win32_Storage_FileSystem`, `Win32_System_Threading`, `Win32_Security`, `Win32_System_Kernel` (for crash dump), `Win32_Foundation` (both).

---

## File Structure

**Create:**

* `rust/nvda_crashdump/Cargo.toml` — crate manifest
* `rust/nvda_crashdump/src/lib.rs` — `write_crash_dump(path, exception_pointers_addr) -> bool` plus magic-bytes unit test
* `rust/nvda_ole/Cargo.toml` — crate manifest
* `rust/nvda_ole/src/lib.rs` — `get_clipboard_text(unknown_addr) -> Result<String>` and `get_user_type(unknown_addr, flags) -> Result<String>`. No Rust unit tests (real COM objects required); manual verification in Step 9.

**Modify:**

* `rust/Cargo.toml` — add the two new members to the workspace
* `rust/nvda_python/Cargo.toml` — add path dependencies on the two new crates
* `rust/nvda_python/src/lib.rs` — add three new `#[pyfunction]` wrappers (writeCrashDump, getOleClipboardText, getOleUserType) and two new `#[pymodule]` submodules (`crashdump`, `ole`)
* `source/utils/_crashHandler.py:182` — switch `NVDAHelper.localLib.writeCrashDump(...)` → `nvdaRust.crashdump.writeCrashDump(...)`
* `source/NVDAObjects/window/edit.py:825,835` — switch the two `NVDAHelper.localLib` calls to `nvdaRust.ole`
* `source/NVDAHelper/localLib.py` — delete the `getOleClipboardText`, `getOleUserType`, and `writeCrashDump` ctypes definitions
* `nvdaHelper/local/sconscript` — remove `oleUtils.cpp` and `crashDump.cpp` from the source list; remove `dbghelp.lib` from `LIBS` (oleUtils only depends on libs already needed elsewhere)
* `nvdaHelper/local/nvdaHelperLocal.def` — remove the three exports (`getOleClipboardText`, `getOleUserType`, `writeCrashDump`)

**Delete:**

* `nvdaHelper/local/oleUtils.cpp` (82 lines)
* `nvdaHelper/local/crashDump.cpp` (52 lines)

---

## Task 1: Create the `nvda_crashdump` crate skeleton

**Files:**

* Create: `rust/nvda_crashdump/Cargo.toml`
* Modify: `rust/Cargo.toml`

* [ ] **Step 1: Write `rust/nvda_crashdump/Cargo.toml`**

```toml
[package]
name = "nvda_crashdump"
version = "0.1.0"
edition = "2021"

[dependencies.windows]
version = "0.58"
features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Storage_FileSystem",
    "Win32_System_Diagnostics_Debug",
    "Win32_System_Kernel",
    "Win32_System_Memory",
    "Win32_System_Threading",
]
```

* [ ] **Step 2: Add the crate to the workspace**

In `rust/Cargo.toml`, change:

```toml
members = ["nvda_core", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
```

to:

```toml
members = ["nvda_core", "nvda_crashdump", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
```

* [ ] **Step 3: Verify workspace recognizes the new crate**

Run: `cd rust && cargo check -p nvda_crashdump 2>&1`
Expected: `error[E0601]: main function not found ... lib.rs not found` OR `Finished ...` — either is OK; we just want cargo to recognize the package. If the error is about a missing `src/lib.rs`, that's expected since we haven't written it yet.

---

## Task 2: TDD `write_crash_dump` in `nvda_crashdump`

**Rationale for the test:** Calling `MiniDumpWriteDump` with `pExceptionParam = NULL` is valid per the WinAPI docs and produces a smaller but well-formed dump. We verify the file exists and starts with the `MDMP` magic (4-byte signature `0x504D444D` little-endian) — that proves we successfully invoked the API and wrote a real minidump, without needing to trigger an actual crash.

**Files:**

* Create: `rust/nvda_crashdump/src/lib.rs`

* [ ] **Step 1: Write the failing test**

Create `rust/nvda_crashdump/src/lib.rs` with the test only (no implementation):

```rust
/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_write_dump_creates_minidump_file() {
        let tmp = std::env::temp_dir().join(format!(
            "nvda_test_dump_{}.dmp",
            std::process::id()
        ));
        let path = tmp.to_string_lossy().to_string();
        let _ = fs::remove_file(&tmp);

        // exception_pointers = 0 (NULL) — valid; produces a dump without exception info
        let ok = write_crash_dump(&path, 0);

        assert!(ok, "write_crash_dump returned false");
        let bytes = fs::read(&tmp).expect("dump file not created");
        assert!(bytes.len() > 32, "dump file is suspiciously small");
        // MINIDUMP_HEADER.Signature = 'MDMP' (0x504D444D LE)
        assert_eq!(&bytes[0..4], b"MDMP", "minidump magic missing");
        let _ = fs::remove_file(&tmp);
    }
}
```

* [ ] **Step 2: Run test to verify it fails to compile**

Run: `cd rust && cargo test -p nvda_crashdump 2>&1 | tail -10`
Expected: compilation error `cannot find function 'write_crash_dump' in this scope`

* [ ] **Step 3: Implement `write_crash_dump`**

Replace `rust/nvda_crashdump/src/lib.rs` with:

```rust
/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpNormal, MiniDumpWriteDump, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

/// Write a minidump for the current process to `path`.
///
/// `exception_pointers_addr` is the integer value of an `EXCEPTION_POINTERS*`
/// (typically from an UnhandledExceptionFilter callback). Pass 0 if no
/// exception context is available — the resulting dump is smaller but still
/// valid.
///
/// Returns true on success.
pub fn write_crash_dump(path: &str, exception_pointers_addr: usize) -> bool {
    // Encode path as UTF-16 with null terminator
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);

    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let handle = match handle {
        Ok(h) if !h.is_invalid() => h,
        _ => return false,
    };

    let mut mdei = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: exception_pointers_addr as *mut EXCEPTION_POINTERS,
        ClientPointers: false.into(),
    };
    let mdei_ptr: Option<*const MINIDUMP_EXCEPTION_INFORMATION> =
        if exception_pointers_addr != 0 { Some(&mdei) } else { None };

    let ok = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            handle,
            MiniDumpNormal,
            mdei_ptr.map(|p| p as *const _).map_or(std::ptr::null(), |p| p) as *const _,
            std::ptr::null(),
            std::ptr::null(),
        )
    };

    let _ = unsafe { CloseHandle(handle) };
    let _ = mdei; // keep alive until after MiniDumpWriteDump returns
    ok.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_write_dump_creates_minidump_file() {
        let tmp = std::env::temp_dir().join(format!(
            "nvda_test_dump_{}.dmp",
            std::process::id()
        ));
        let path = tmp.to_string_lossy().to_string();
        let _ = fs::remove_file(&tmp);

        let ok = write_crash_dump(&path, 0);

        assert!(ok, "write_crash_dump returned false");
        let bytes = fs::read(&tmp).expect("dump file not created");
        assert!(bytes.len() > 32, "dump file is suspiciously small");
        assert_eq!(&bytes[0..4], b"MDMP", "minidump magic missing");
        let _ = fs::remove_file(&tmp);
    }
}
```

**Implementation note for the engineer:** The exact `MiniDumpWriteDump` argument types in `windows` 0.58 may need a small adjustment — the function signature uses `Option<*const MINIDUMP_EXCEPTION_INFORMATION>` in some versions and bare pointer in others. If the code above doesn't compile, simplify to:

```rust
let mdei_param = if exception_pointers_addr != 0 { Some(&mdei as *const _) } else { None };
let ok = unsafe {
    MiniDumpWriteDump(
        GetCurrentProcess(), GetCurrentProcessId(), handle,
        MiniDumpNormal,
        mdei_param,
        None, None,
    )
};
```

Match whatever the compiler tells you the parameter types are. Don't redesign the API around it — just satisfy the signature.

* [ ] **Step 4: Run test to verify it passes**

Run: `cd rust && cargo test -p nvda_crashdump 2>&1 | tail -10`
Expected: `test tests::test_write_dump_creates_minidump_file ... ok`, `test result: ok. 1 passed`

* [ ] **Step 5: Commit**

```bash
git add rust/nvda_crashdump/ rust/Cargo.toml rust/Cargo.lock
git commit -m "Add nvda_crashdump crate with write_crash_dump"
```

---

## Task 3: Create the `nvda_ole` crate skeleton

**Files:**

* Create: `rust/nvda_ole/Cargo.toml`
* Modify: `rust/Cargo.toml`

* [ ] **Step 1: Write `rust/nvda_ole/Cargo.toml`**

```toml
[package]
name = "nvda_ole"
version = "0.1.0"
edition = "2021"

[dependencies.windows-core]
version = "0.58"

[dependencies.windows]
version = "0.58"
features = [
    "Win32_Foundation",
    "Win32_Graphics_Gdi",
    "Win32_System_Com",
    "Win32_System_Com_StructuredStorage",
    "Win32_System_Ole",
    "Win32_System_Memory",
]
```

(The implementer may need to add more features as the compiler demands. `STGMEDIUM` and `ReleaseStgMedium` are gated on `Win32_Graphics_Gdi` + `Win32_System_Com_StructuredStorage`. `CF_UNICODETEXT` lives in `Win32_System_Ole`, not `Win32_System_DataExchange`, in this version.)

* [ ] **Step 2: Add the crate to the workspace**

In `rust/Cargo.toml`, update members to include `nvda_ole`:

```toml
members = ["nvda_core", "nvda_crashdump", "nvda_ole", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
```

* [ ] **Step 3: Verify workspace recognizes it**

Run: `cd rust && cargo check -p nvda_ole 2>&1 | tail -5`
Expected: error about missing `src/lib.rs` (we haven't written it yet) — OK.

---

## Task 4: Implement `get_clipboard_text` in `nvda_ole`

**Why no Rust unit test:** Both functions take a real `IUnknown*`. Constructing one in Rust without a real COM object means writing a custom COM server (lots of boilerplate, low value). Manual end-to-end verification in WLM/Outlook (Step 9) plus the existing C++ behavior being replaced verbatim is the validation. The C++ originals had no automated tests either.

**Files:**

* Create: `rust/nvda_ole/src/lib.rs`

* [ ] **Step 1: Write `rust/nvda_ole/src/lib.rs`**

```rust
/*
A part of NonVisual Desktop Access (NVDA)
Copyright (C) 2026 NV Access Limited
This file may be used under the terms of the GNU General Public License,
version 2 or later, as modified by the NVDA license. For full terms and any
additional permissions, see the NVDA license file:
https://github.com/nvaccess/nvda/blob/master/copying.txt
*/

use windows::core::Interface;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOINTERFACE};
use windows::Win32::System::Com::{
    IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::CF_UNICODETEXT;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{IOleObject, ReleaseStgMedium};
use windows_core::IUnknown;

/// Result type used by both functions: `Ok(text)` on success, `Err(hresult)`
/// where the inner `i32` is the raw HRESULT for the Python caller to map to
/// a `WindowsError`.
pub type OleResult = Result<String, i32>;

/// Borrow an IUnknown from a raw pointer address (e.g. passed from Python via
/// `ctypes.cast(comObj, ctypes.c_void_p).value`). Returns `None` for null.
///
/// SAFETY: caller must ensure `addr` is a valid IUnknown pointer that outlives
/// the returned reference. We do NOT take ownership (no AddRef/Release).
unsafe fn borrow_iunknown(addr: usize) -> Option<IUnknown> {
    if addr == 0 {
        return None;
    }
    // from_raw takes ownership and would Release on drop. We want borrowed
    // semantics, so we AddRef first to balance the implicit Release.
    let raw = addr as *mut std::ffi::c_void;
    let iface = IUnknown::from_raw_borrowed(&raw)?;
    Some(iface.clone())
}

pub fn get_clipboard_text(unknown_addr: usize) -> OleResult {
    let unknown = unsafe { borrow_iunknown(unknown_addr) }.ok_or(E_INVALIDARG.0)?;
    let data_object: IDataObject = unknown.cast().map_err(|_| E_NOINTERFACE.0)?;

    let format = FORMATETC {
        cfFormat: CF_UNICODETEXT.0 as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };

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
            ReleaseStgMedium(&mut { medium });
            return Err(E_FAIL.0);
        }
        // Read NUL-terminated wide string
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

**Engineer notes:**

* The exact spelling of `medium.u.hGlobal` vs `medium.Anonymous.hGlobal` and the borrow rules around `STGMEDIUM` differ slightly between `windows` 0.58 patch versions. If you hit field-name errors, run `cargo doc --open -p windows` and look up `STGMEDIUM` for this exact version.
* `ReleaseStgMedium` takes `*mut STGMEDIUM`. The `&mut { medium }` pattern moves the value into a fresh binding so we can take a mutable reference. If the borrow checker complains, switch to `let mut medium = medium; ReleaseStgMedium(&mut medium);` after the conditional checks.
* Don't add error-handling sophistication beyond mapping HRESULT codes through. The C++ original just returned the HRESULT — match that contract.

* [ ] **Step 2: Verify it compiles**

Run: `cd rust && cargo check -p nvda_ole 2>&1 | tail -10`
Expected: `Finished ...` with no errors. Warnings about unused `IOleObject` import are fine — Step 5 uses it.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ole/Cargo.toml rust/nvda_ole/src/lib.rs rust/Cargo.toml rust/Cargo.lock
git commit -m "Add nvda_ole crate with get_clipboard_text"
```

---

## Task 5: Add `get_user_type` to `nvda_ole`

**Files:**

* Modify: `rust/nvda_ole/src/lib.rs`

* [ ] **Step 1: Append `get_user_type` to `rust/nvda_ole/src/lib.rs`**

Add at the bottom of the file (after `get_clipboard_text`):

```rust
pub fn get_user_type(unknown_addr: usize, flags: u32) -> OleResult {
    use windows::Win32::System::Com::{CoTaskMemFree};

    let unknown = unsafe { borrow_iunknown(unknown_addr) }.ok_or(E_INVALIDARG.0)?;
    let ole_object: IOleObject = unknown.cast().map_err(|_| E_NOINTERFACE.0)?;

    let ole_str = unsafe { ole_object.GetUserType(flags) }.map_err(|e| e.code().0)?;
    if ole_str.is_null() {
        return Err(E_FAIL.0);
    }

    // ole_str is a PWSTR allocated with CoTaskMemAlloc; we own it
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

**Note:** The C++ original calls `CoGetMalloc` then `IMalloc::Free`. `CoTaskMemFree` is the documented equivalent and is what `windows-rs` provides directly.

* [ ] **Step 2: Verify it compiles**

Run: `cd rust && cargo check -p nvda_ole 2>&1 | tail -5`
Expected: `Finished ...`

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ole/src/lib.rs
git commit -m "nvda_ole: add get_user_type"
```

---

## Task 6: Wire both crates into `nvda_python`

**Files:**

* Modify: `rust/nvda_python/Cargo.toml`
* Modify: `rust/nvda_python/src/lib.rs`

* [ ] **Step 1: Add path dependencies**

In `rust/nvda_python/Cargo.toml`, find the `[dependencies]` section and add (alphabetical with existing entries):

```toml
nvda_crashdump = { path = "../nvda_crashdump" }
nvda_ole = { path = "../nvda_ole" }
```

So the full `[dependencies]` block should now contain `nvda_crashdump`, `nvda_ole`, `nvda_text`, `nvda_tones`, `nvda_wasapi`, `pyo3`, `windows`.

* [ ] **Step 2: Add PyO3 wrappers**

In `rust/nvda_python/src/lib.rs`, add the following pyfunctions immediately before the `tones_mod` `#[pymodule]` block:

```rust
// Crash dump — thin wrapper around nvda_crashdump.
#[pyfunction]
#[pyo3(name = "writeCrashDump")]
fn write_crash_dump(path: &str, exception_pointers: usize) -> bool {
    nvda_crashdump::write_crash_dump(path, exception_pointers)
}

// OLE helpers — thin wrappers around nvda_ole. The COM IUnknown is passed
// from Python as an integer pointer:
//     ptr = ctypes.cast(comObj, ctypes.c_void_p).value
// HRESULT errors are mapped to PyOSError.
#[pyfunction]
#[pyo3(name = "getOleClipboardText")]
fn get_ole_clipboard_text(unknown: usize) -> PyResult<String> {
    nvda_ole::get_clipboard_text(unknown).map_err(|hr| {
        pyo3::exceptions::PyOSError::new_err(format!("HRESULT 0x{:08x}", hr as u32))
    })
}

#[pyfunction]
#[pyo3(name = "getOleUserType")]
fn get_ole_user_type(unknown: usize, flags: u32) -> PyResult<String> {
    nvda_ole::get_user_type(unknown, flags).map_err(|hr| {
        pyo3::exceptions::PyOSError::new_err(format!("HRESULT 0x{:08x}", hr as u32))
    })
}
```

* [ ] **Step 3: Add submodule declarations**

In the same file, add two new `#[pymodule]` blocks immediately before the `text_mod` block:

```rust
#[pymodule]
#[pyo3(name = "crashdump")]
mod crashdump_mod {
    #[pymodule_export]
    use super::write_crash_dump;
}

#[pymodule]
#[pyo3(name = "ole")]
mod ole_mod {
    #[pymodule_export]
    use super::get_ole_clipboard_text;
    #[pymodule_export]
    use super::get_ole_user_type;
}
```

* [ ] **Step 4: Register the new submodules in `nvda_rust`**

In the `mod nvda_rust { ... }` `#[pymodule]` block (the outer registration), add (alphabetical with existing exports):

```rust
    #[pymodule_export]
    use super::crashdump_mod;
    #[pymodule_export]
    use super::ole_mod;
```

* [ ] **Step 5: Build the wheel and confirm import**

Run: `cd rust/nvda_python && uvx maturin develop 2>&1 | tail -5`
Expected: `Installed nvdaRust-0.1.0`

Run: `cd <project root> && uv run python -c "import nvdaRust; print(dir(nvdaRust.crashdump)); print(dir(nvdaRust.ole))"`
Expected output contains `writeCrashDump` and (`getOleClipboardText`, `getOleUserType`).

* [ ] **Step 6: Smoke-test crashDump end-to-end from Python**

Run:

```bash
uv run python -c "
import nvdaRust, os, tempfile
p = os.path.join(tempfile.gettempdir(), 'nvda_smoke.dmp')
ok = nvdaRust.crashdump.writeCrashDump(p, 0)
print('ok=', ok, 'size=', os.path.getsize(p))
os.remove(p)
"
```

Expected: `ok= True size= <some number > 1000>`

* [ ] **Step 7: Commit**

```bash
git add rust/nvda_python/Cargo.toml rust/nvda_python/src/lib.rs rust/Cargo.lock
git commit -m "nvda_python: expose nvdaRust.crashdump and nvdaRust.ole"
```

---

## Task 7: Switch `_crashHandler.py` to use `nvdaRust.crashdump`

**Files:**

* Modify: `source/utils/_crashHandler.py:182`

* [ ] **Step 1: Update the call site**

In `source/utils/_crashHandler.py`, find the line:

```python
if not NVDAHelper.localLib.writeCrashDump(dumpPath, exceptionInfo):
```

and replace with:

```python
import nvdaRust
# exceptionInfo is a ctypes.POINTER(EXCEPTION_RECORD); pass its address as an int.
exceptionInfoAddr = ctypes.cast(exceptionInfo, ctypes.c_void_p).value or 0
if not nvdaRust.crashdump.writeCrashDump(dumpPath, exceptionInfoAddr):
```

The `import nvdaRust` should go up at the top of the file with the other imports — move it there during the edit, then leave only the address-extraction + call at line 182.

* [ ] **Step 2: Verify the file still parses**

Run: `uv run python -c "import utils._crashHandler" 2>&1`
Expected: no output (success).

* [ ] **Step 3: Commit**

```bash
git add source/utils/_crashHandler.py
git commit -m "_crashHandler: use nvdaRust.crashdump.writeCrashDump"
```

---

## Task 8: Switch `edit.py` to use `nvdaRust.ole`

**Files:**

* Modify: `source/NVDAObjects/window/edit.py:825,835`

* [ ] **Step 1: Replace both call sites**

In `source/NVDAObjects/window/edit.py`, the existing block at lines 822–841:

```python
		# Windows Live Mail exposes the label via the embedded object's data (IDataObject)
		text = BSTR()
		try:
			NVDAHelper.localLib.getOleClipboardText(o, ctypes.byref(text))
		except WindowsError:
			pass
		else:
			label = text.value
		if label:
			return label
		# As a final fallback (e.g. could not get display model text for Outlook Express), use the embedded object's user type (e.g. "recipient").
		userType = BSTR()
		try:
			NVDAHelper.localLib.getOleUserType(o, 0, ctypes.byref(userType))
		except WindowsError:
			pass
		else:
			label = userType.value
		if label:
			return label
```

becomes:

```python
		import nvdaRust

		# Windows Live Mail exposes the label via the embedded object's data (IDataObject)
		oAddr = ctypes.cast(o, ctypes.c_void_p).value or 0
		try:
			label = nvdaRust.ole.getOleClipboardText(oAddr)
		except OSError:
			pass
		if label:
			return label
		# As a final fallback (e.g. could not get display model text for Outlook Express), use the embedded object's user type (e.g. "recipient").
		try:
			label = nvdaRust.ole.getOleUserType(oAddr, 0)
		except OSError:
			pass
		if label:
			return label
```

Notes for the engineer:

* `nvdaRust.ole.*` raises `OSError` (PyOSError) on HRESULT failure, replacing the previous `WindowsError`-via-restype-`HRESULT` behavior. `WindowsError` is an alias for `OSError` on Python ≥3.3, so this is a behavioral no-op for callers.
* The functions return `str` directly (not BSTR-via-out-param), so the `text = BSTR()` / `text.value` ceremony goes away.

* [ ] **Step 2: Verify the file still parses**

Run: `uv run python -c "import NVDAObjects.window.edit" 2>&1`
Expected: no output (success). If you get `ModuleNotFoundError`, that's likely a missing-NVDA-runtime issue unrelated to your edit — try `cd source && uv run python -c "import NVDAObjects.window.edit"`.

* [ ] **Step 3: Commit**

```bash
git add source/NVDAObjects/window/edit.py
git commit -m "edit.py: use nvdaRust.ole for clipboard text and user type"
```

---

## Task 9: Manual end-to-end verification of the OLE path

There's no automated test for this — the Python integration would need a real OLE-embedded object. Validate by hand before removing the C++ side.

* [ ] **Step 1: Build NVDA with the new wheel**

Run: `scons.bat source --all-cores` (or `.\scons.bat source --all-cores` on Windows in a worktree).

* [ ] **Step 2: Open Outlook Express / Windows Live Mail (or any HTML email client with embedded objects)**

If neither is available, open Outlook and view an email with an attachment — the attachment row exposes `IOleObject::GetUserType`. Tab to the attachment, then Down/Up — NVDA should announce the attachment's user type ("Recipient", "Attachment", etc.).

* [ ] **Step 3: Verify behavior is identical to master**

If you have a master build to compare against, both should announce the same labels for the same items. If the new build announces empty/wrong labels and master is correct, **stop and investigate** before proceeding to Task 10 — the OLE port has a bug.

If you can't find a way to trigger this code path (Windows Live Mail is EOL, embedded OLE is rare today), document it: the manual test couldn't be performed. The Rust functions still need to compile and be importable from Python, which Tasks 6.5 and 6.6 verified.

* [ ] **Step 4: Mark this task complete in the plan**

No commit — this is a verification gate.

---

## Task 10: Remove ctypes definitions from `localLib.py`

**Files:**

* Modify: `source/NVDAHelper/localLib.py`

* [ ] **Step 1: Delete the three ctypes blocks**

Delete the following blocks from `source/NVDAHelper/localLib.py`:

Lines around 384–397 (the `getOleClipboardText` and `getOleUserType` definitions):

```python
getOleClipboardText = dll.getOleClipboardText
getOleClipboardText.restype = HRESULT
getOleClipboardText.argtypes = (
	POINTER(IUnknown),
	POINTER(BSTR),
)

getOleUserType = dll.getOleUserType
getOleUserType.restype = HRESULT
getOleUserType.argtypes = (
	POINTER(IUnknown),
	DWORD,
	POINTER(BSTR),
)
```

Lines around 533–544 (the `writeCrashDump` definition + docstring):

```python
writeCrashDump = dll.writeCrashDump
"""
Writes a crash dump to the specified path.
...
"""
writeCrashDump.argtypes = (
	c_wchar_p,
	c_void_p,
)
writeCrashDump.restype = bool
```

* [ ] **Step 2: Verify localLib.py still imports**

Run: `uv run python -c "from NVDAHelper import localLib; print('OK')" 2>&1`
Expected: `OK` (or a benign import-time warning).

* [ ] **Step 3: Commit**

```bash
git add source/NVDAHelper/localLib.py
git commit -m "localLib: drop ctypes defs for ported OLE/crashDump functions"
```

---

## Task 11: Remove the C++ files and wire-up

**Files:**

* Modify: `nvdaHelper/local/sconscript`
* Modify: `nvdaHelper/local/nvdaHelperLocal.def`
* Delete: `nvdaHelper/local/oleUtils.cpp`
* Delete: `nvdaHelper/local/crashDump.cpp`

* [ ] **Step 1: Remove sources from `sconscript`**

In `nvdaHelper/local/sconscript`, find the source list and delete the lines `"oleUtils.cpp",` and `"crashDump.cpp",`. If `crashDump.cpp` brought in a `dbghelp` library entry in `LIBS`, delete that too — search for `"dbghelp"` in the file and remove the line if it's the only consumer (it should be — no other file in `nvdaHelper/local/` references `MiniDumpWriteDump`).

* [ ] **Step 2: Remove exports from `.def`**

In `nvdaHelper/local/nvdaHelperLocal.def`, delete the three lines:

```
	getOleClipboardText
	getOleUserType
	writeCrashDump
```

* [ ] **Step 3: Delete the .cpp files**

```bash
git rm nvdaHelper/local/oleUtils.cpp nvdaHelper/local/crashDump.cpp
```

* [ ] **Step 4: Rebuild nvdaHelper to confirm it links without the removed exports**

Run: `.\scons.bat source --all-cores 2>&1 | tail -20`
Expected: build succeeds. If it fails with "unresolved symbol getOleClipboardText" or similar, you missed a Python-side caller — grep the source tree:

```bash
grep -rn "getOleClipboardText\|getOleUserType\|writeCrashDump" source/ tests/
```

Any hit other than your own new `nvdaRust.*` call sites is a remaining ctypes consumer.

* [ ] **Step 5: Run the full unit test suite**

Run: `./rununittests.bat 2>&1 | tail -10`
Expected: `Ran <N> tests in <T>s`, `OK`. If anything regresses, fix it before committing.

* [ ] **Step 6: Commit**

```bash
git add nvdaHelper/local/sconscript nvdaHelper/local/nvdaHelperLocal.def
git commit -m "Remove ported C++ OLE and crashDump sources"
```

---

## Task 12: Final sanity sweep and push

* [ ] **Step 1: Confirm clean tree (modulo expected submodule dirt)**

Run: `git status -s`
Expected: only the unstaged submodule entries we already know about (`include/detours`, `include/nvda-mathcat`, `miscDeps`).

* [ ] **Step 2: Re-run cargo tests across the workspace**

Run: `cd rust && cargo test 2>&1 | tail -10`
Expected: all tests pass (including the new `nvda_crashdump` test, plus the existing `nvda_text` 20 tests, etc.).

* [ ] **Step 3: Show the commit log for review**

Run: `git log --oneline origin/master..HEAD`
Expected: ~7 new commits on top of where the branch started, telling a clean story (Add nvda_crashdump → Add nvda_ole → expose in nvda_python → switch_crashHandler → switch edit.py → drop ctypes defs → remove C++ sources).

* [ ] **Step 4: Push**

```bash
git push origin HEAD
```

Per project convention, do NOT open a PR automatically — wait for the user to eyeball the diff first.

---

## Out of scope

* Porting `screenCurtain.cpp`, `dllImportTableHooks.cpp`, `nvdaHelperLocal.cpp`, RPC plumbing, or anything else from `nvdaHelper/local/`. Each of those merits its own plan.
* Changing the calling convention or behavior of the three Python call sites beyond what's necessary to use the new APIs (e.g. don't refactor `_getEmbedTextFromOleObject` while you're in there).
* Adding tests for `getOleUserType` / `getOleClipboardText` that require constructing a real COM `IDataObject`/`IOleObject` from scratch — defer until there's a clear pattern for that across the codebase.
