# Rust input hooks de-risk — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `nvdaHelper/remote/inputLangChange.cpp` (45 LOC, single Windows-hook handler) to a new `nvda_input_hooks` Rust crate built as a `staticlib` and linked into `nvdaHelperRemote.dll` by SCons. This is the **first Rust code that runs inside other processes** via DLL injection — the value of this branch is the build-integration pattern, not the line count.

**Architecture:** New crate `rust/nvda_input_hooks` with `crate-type = ["staticlib"]`. SCons gains a small builder that invokes `cargo build` and produces a `.lib` artifact, which is added to the `LIBS` list of `nvdaHelperRemote.dll`. The Rust crate exposes `inputLangChange_inProcess_initialize` and `_terminate` as `extern "C"` symbols matching the existing C++ declarations in `nvdaHelper/remote/inputLangChange.h`. Inside, it calls back into existing C++ infrastructure (`registerWindowsHook`, `nvdaControllerInternal_inputLangChangeNotify`) declared as `extern "C"` from Rust — no IDL/RPC layer is rewritten. The C++ caller (`inProcess.cpp`) is unchanged.

**Tech Stack:** Rust 2021, `windows` 0.58 (`Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_TextServices`, `Win32_UI_Input_KeyboardAndMouse`), SCons subprocess-based cargo invocation, MSVC linker integrating Rust `staticlib` into a C++ `SharedLibrary`.

---

## Why de-risk?

Up to this point every Rust crate in this codebase ships as a Python extension (`.pyd`) loaded into NVDA's main process via maturin/PyO3. `nvdaHelperRemote.dll` is different on every axis:

* It's a plain Win32 DLL, no Python.
* It's **injected** into target processes (Word, Chrome, every browser tab, OS shell windows).
* It's built by SCons as a `SharedLibrary`, not by maturin.
* Its calling-convention boundaries are C++ (or `extern "C"`), not PyO3.

Before committing to bigger ports inside `nvdaHelper/remote/` (e.g., `textFromIAccessible`, `displayModel`, eventually VBuf or `gdiHooks`), this plan validates **end-to-end**:

1. Cargo `staticlib` artifact + SCons link integration.
2. Rust function exported as a C-ABI symbol that C++ calls at startup.
3. Rust function registering a Windows hook callback (`extern "system" fn`) that's executed by the OS message pump in the target process.
4. Rust calling a MIDL-generated RPC stub (`nvdaControllerInternal_inputLangChangeNotify`) declared as `extern "C"`.
5. The resulting Rust code surviving DLL injection into arbitrary host processes.

If any of these break, fix them here on a 22-LOC port — not on a 1000-LOC one. Once this lands, the cargo-staticlib + SCons-link pattern is reusable for every future `nvdaHelper/remote/*.cpp` port.

---

## Scope explicitly excluded

* **`typedCharacter.cpp`** is intentionally NOT in scope. It exposes a shared mutable global `typedCharacter_window` that `ime.cpp:499` and `tsf.cpp:498` also write. Porting it cleanly requires either `extern "C" static mut` from Rust (ergonomic warts) or refactoring the shared-state contract — both worth doing once the pattern is proven on the simpler case.
* **No port of `log.cpp`, `injection.cpp`, `inProcess.cpp`, `rpcSrv.cpp`** or any other infrastructure file. We're a passenger in `nvdaHelperRemote.dll`, not its driver.
* **No Rust unit tests for the hook logic itself.** A Windows hook callback can only be exercised by injecting the DLL into a real host process and triggering a `WM_INPUTLANGCHANGE` message. Manual verification (Task 8) is the test.

---

## File Structure

**Create:**

* `rust/nvda_input_hooks/Cargo.toml` — staticlib crate manifest. Depends on `windows` 0.58 with the Win32 features needed for hooks + keyboard layout.
* `rust/nvda_input_hooks/src/lib.rs` — single file containing:
  * `extern "C"` declarations for `registerWindowsHook`, `unregisterWindowsHook`, `nvdaControllerInternal_inputLangChangeNotify` (resolved at link time inside `nvdaHelperRemote.dll`).
  * The `extern "system"` hook callback mirroring `inputLangChange_callWndProcHook`.
  * Two `#[no_mangle] pub extern "C"` functions: `inputLangChange_inProcess_initialize` and `inputLangChange_inProcess_terminate`.

**Modify:**

* `rust/Cargo.toml` — add `nvda_input_hooks` to workspace `members`, alphabetical with the existing crates (`nvda_core, nvda_crashdump, nvda_input_hooks, nvda_ole, nvda_python, nvda_screen_curtain, nvda_text, nvda_tones, nvda_wasapi`).
* `nvdaHelper/remote/sconscript` — add inline cargo invocation that builds the staticlib and adds the resulting `.lib` to the `LIBS` list of the `nvdaHelperRemote` `SharedLibrary`. Remove `"inputLangChange.cpp"` from the `source` list.
* `.gitignore` — confirm `rust/target/` is already covered (it is — added in commit `e9d5bf9aa`). No change expected; verify only.

**Delete:**

* `nvdaHelper/remote/inputLangChange.cpp` — the C++ original (45 lines).

**Keep unchanged:**

* `nvdaHelper/remote/inputLangChange.h` — still needed: it declares the two functions Rust now provides AND it defines `EVENT_INPUTLANGCHANGE 0x1001` which `tsf.cpp:537,560` consumes via `#include "inputLangChange.h"`. The header's contract is what makes the swap transparent to callers.
* `nvdaHelper/remote/inProcess.cpp:48-62` — calls `inputLangChange_inProcess_initialize/terminate` exactly as before. The Rust symbols satisfy the same `extern "C"` declaration in the header.

---

## Working assumptions (verify if you hit weirdness)

1. **MSVC tolerates Rust staticlibs.** The Rust `windows-msvc` toolchain produces `.lib` files compatible with MSVC's linker. The Rust libstd dependency adds ~600 KB to the final DLL. We accept this — for a single 22-LOC function it's overhead-heavy, but the de-risking value is in the integration path itself, not size.
2. **No CRT mismatch.** Both nvdaHelperRemote.dll's MSVC build and Rust's MSVC build target the same C runtime (`msvcrt` or universal CRT, whichever NVDA uses). If you see "LNK4098: defaultlib 'libcmt' conflicts with use of other libs" or similar, the fix is to align CRT settings (typically `/MT` vs `/MD`); document the resolution in the commit message.
3. **No GdiplusStartup-style host-process dependency.** `inputLangChange.cpp` only uses `SetWindowsHookEx`/`UnhookWindowsHookEx` (via the `registerWindowsHook` wrapper) and `GetKeyboardLayoutName`. No process-lifetime singletons to worry about.
4. **The MIDL-generated stub `nvdaControllerInternal_inputLangChangeNotify` uses `__cdecl` calling convention** (the `[in]` IDL parameters are `unsigned long, unsigned long, [in,string] wchar_t*`). Rust `extern "C"` matches `__cdecl` on x86 and x86_64. Confirm the IDL file at `nvdaHelper/interfaces/nvdaControllerInternal/nvdaControllerInternal.idl` if you hit calling-convention errors.

---

## Task 1: Create the `nvda_input_hooks` crate skeleton

**Files:**

* Create: `rust/nvda_input_hooks/Cargo.toml`
* Create: `rust/nvda_input_hooks/src/lib.rs`
* Modify: `rust/Cargo.toml`

* [ ] **Step 1: Write `rust/nvda_input_hooks/Cargo.toml`**

```toml
[package]
name = "nvda_input_hooks"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]

[dependencies.windows]
version = "0.58"
features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_TextServices",
]
```

The `crate-type = ["staticlib"]` is the load-bearing line — it tells cargo to produce a `.lib` file (Windows static archive) rather than a Rust `rlib`. Without it, MSVC can't link the result.

* [ ] **Step 2: Add the crate to the workspace**

In `rust/Cargo.toml`, the current `members` list is:

```toml
members = ["nvda_core", "nvda_crashdump", "nvda_ole", "nvda_screen_curtain", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
```

Update to (alphabetical with `nvda_input_hooks` inserted between `nvda_crashdump` and `nvda_ole`):

```toml
members = ["nvda_core", "nvda_crashdump", "nvda_input_hooks", "nvda_ole", "nvda_screen_curtain", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
```

* [ ] **Step 3: Write a minimal `rust/nvda_input_hooks/src/lib.rs`** (just enough to verify the workspace builds)

```rust
/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

//! NVDA Helper Remote: Rust port of Windows-hook-based input event handlers.
//!
//! This crate is built as a `staticlib` and linked into `nvdaHelperRemote.dll`.
//! It is loaded into target processes via DLL injection, so the same code runs
//! inside Word, Chrome, every browser tab, etc. Keep allocations minimal and do
//! not rely on host-process global state.
```

* [ ] **Step 4: Verify cargo recognises the crate and produces a `.lib`**

Run from the project root:

```
cd rust && cargo build -p nvda_input_hooks --release 2>&1 | tail -5
```

Expected: `Compiling nvda_input_hooks v0.1.0` followed by `Finished release [optimized] target(s) in <N>s`. The output `.lib` should land at `rust/target/release/nvda_input_hooks.lib`.

Verify it exists:

```
ls -la rust/target/release/nvda_input_hooks.lib
```

Expected: a non-zero-sized file (~5–50 MB depending on Rust libstd inlining).

* [ ] **Step 5: Commit**

```bash
git add rust/nvda_input_hooks/ rust/Cargo.toml rust/Cargo.lock
git commit -m "Add nvda_input_hooks crate skeleton (staticlib)"
```

---

## Task 2: Wire the cargo build into `nvdaHelper/remote/sconscript`

This is the load-bearing task. We're teaching SCons to invoke cargo, capture the resulting `.lib` path, and add it to the C++ link.

**Files:**

* Modify: `nvdaHelper/remote/sconscript`

* [ ] **Step 1: Read the current sconscript carefully**

Run:

```
cat nvdaHelper/remote/sconscript
```

Note the structure:

* Line 88–123: `source = [...]` list. We will eventually remove `"inputLangChange.cpp"` from this list (Task 6).
* Line 125–139: `libs = [...]` list. We will append a new `cargoStaticLib` File node here in this task.
* Line 141–145: `env.SharedLibrary(target="nvdaHelperRemote", source=source, LIBS=libs)`. We don't touch this.

* [ ] **Step 2: Add the cargo invocation block immediately before the `libs = [...]` definition**

Insert this block after the `source = [...]` list and before `libs = [...]`:

```python
# Build the Rust staticlib for the input-hooks port.
# We invoke cargo via a SCons Command so the build is part of the dependency
# graph: editing rust/nvda_input_hooks/src/lib.rs forces a relink of
# nvdaHelperRemote.dll, and `scons -c` cleans the cargo target directory.
import os
import subprocess

rustCrateDir = Dir("#rust/nvda_input_hooks")
rustWorkspaceDir = Dir("#rust")
rustTargetDir = Dir("#build/rust")
rustOutputLib = rustTargetDir.File("release/nvda_input_hooks.lib")

# Glob source files so SCons re-runs cargo when any .rs or Cargo.toml changes.
rustSources = (
    env.Glob(rustCrateDir.path + "/src/*.rs")
    + [rustCrateDir.File("Cargo.toml"), Dir("#rust").File("Cargo.toml")]
)


def buildCargoStaticLib(target, source, env):
    """Run `cargo build --release -p nvda_input_hooks` and place the .lib
    where SCons expects it."""
    result = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--package",
            "nvda_input_hooks",
            "--target-dir",
            rustTargetDir.abspath,
            "--manifest-path",
            rustWorkspaceDir.File("Cargo.toml").abspath,
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"cargo build failed:\n{result.stderr}")
        return result.returncode
    # cargo produces nvda_input_hooks.lib at <target-dir>/release/.
    # Confirm the file landed where we expect.
    if not os.path.exists(target[0].abspath):
        print(f"cargo built successfully but {target[0].abspath} was not produced")
        return 1
    return 0


cargoStaticLib = env.Command(
    rustOutputLib,
    rustSources,
    buildCargoStaticLib,
)
```

**Engineer notes:**

* `Dir("#rust/...")` is SCons syntax for a path relative to the project root (the `#` anchor). This avoids brittle relative paths.
* `--target-dir build/rust` puts the cargo output under SCons's build tree, which keeps `scons -c` cleanups predictable. Cargo's normal `target/` directory is gitignored, but for SCons integration we redirect it.
* The `rustSources` list ensures SCons re-runs cargo when any `.rs` file or `Cargo.toml` changes. Cargo's own incremental builds make repeated invocations cheap.
* We deliberately don't capture cargo's stdout — it's verbose and has its own progress UI. We capture stderr for error reporting.

* [ ] **Step 3: Add the resulting .lib to the LIBS list**

In the `libs = [...]` block, append `cargoStaticLib` as the last entry (i.e., the line that currently ends `detoursLib,` should be followed by a new line `cargoStaticLib,` before the `]`). The block becomes:

```python
libs = [
    "user32",
    "ole32",
    "rpcrt4",
    "shlwapi",
    "oleaut32",
    "oleacc",
    "usp10",
    "imm32",
    "advapi32",
    "version",
    "DbgHelp",
    "gdi32",
    detoursLib,
    cargoStaticLib,
]
```

The Rust libstd brings in some Win32 dependencies (in particular `bcrypt.dll` for the secure RNG, `ntdll.dll` for sync primitives). MSVC will resolve those automatically since they're in the default lib search path; if you see `LNK2019: unresolved external symbol` for a `Bcrypt*` or `Nt*` function, append `"bcrypt"` and/or `"ntdll"` to the libs list.

* [ ] **Step 4: Verify the SCons build still produces nvdaHelperRemote.dll without source-list changes**

Run from the project root (using `scons.bat` because we're in a worktree with the venv setup):

```
.\scons.bat source --all-cores 2>&1 | tail -30
```

Expected: cargo runs once (or skips if you already built it in Task 1), then SCons builds `source/lib/x64/nvdaHelperRemote.dll` successfully. Look for `Linking nvdaHelperRemote.dll` and `scons: done building targets.` in the output.

If the link fails:

* `LNK2019: unresolved external symbol __imp_BCrypt*` → add `"bcrypt"` to libs
* `LNK4098: defaultlib 'libcmt' conflicts` → CRT mismatch; check the cargo profile and possibly add `[profile.release] panic = "abort"` to the workspace Cargo.toml or `RUSTFLAGS="-C target-feature=+crt-static"` to the cargo invocation. Match whichever CRT mode `nvdaHelperRemote.dll` uses.
* Anything else → escalate before "fixing" it; the integration model may need rethinking.

* [ ] **Step 5: Commit**

```bash
git add nvdaHelper/remote/sconscript
git commit -m "sconscript: build nvda_input_hooks staticlib and link it into nvdaHelperRemote"
```

At this point `nvdaHelperRemote.dll` contains an unused Rust `.lib`. The link works, but no symbols are referenced yet. That's the integration baseline — proceed.

---

## Task 3: Implement the hook logic in Rust

**Files:**

* Modify: `rust/nvda_input_hooks/src/lib.rs`

* [ ] **Step 1: Replace the placeholder `lib.rs` with the real implementation**

```rust
/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

//! NVDA Helper Remote: Rust port of Windows-hook-based input event handlers.
//!
//! This crate is built as a `staticlib` and linked into `nvdaHelperRemote.dll`.
//! It is loaded into target processes via DLL injection, so the same code runs
//! inside Word, Chrome, every browser tab, etc. Keep allocations minimal and do
//! not rely on host-process global state.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutName, KL_NAMELENGTH};
use windows::Win32::UI::WindowsAndMessaging::{CWPSTRUCT, WH_CALLWNDPROC, WM_INPUTLANGCHANGE};

// Symbols resolved at link time inside nvdaHelperRemote.dll.
//
// `registerWindowsHook` and `unregisterWindowsHook` are NVDA helpers
// (declared in `nvdaHelperRemote.h`) that wrap `SetWindowsHookEx` /
// `UnhookWindowsHookEx` and track installed hooks for cleanup at injection
// teardown.
//
// `nvdaControllerInternal_inputLangChangeNotify` is a MIDL-generated client
// stub; the IDL is at
// `nvdaHelper/interfaces/nvdaControllerInternal/nvdaControllerInternal.idl`.
//
// `isTSFThread()` is defined in `nvdaHelper/remote/tsf.cpp`. Declaring it
// here keeps Rust as a peer of the C++ files inside nvdaHelperRemote.dll.
// The C++ version returns `bool` which is one byte; we use `u8` for ABI
// safety and compare against 0.
extern "C" {
    fn registerWindowsHook(
        hook_type: i32,
        proc: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
    );
    fn unregisterWindowsHook(
        hook_type: i32,
        proc: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
    );
    fn nvdaControllerInternal_inputLangChangeNotify(
        thread_id: u32,
        hkl: u32,
        layout_name: *const u16,
    ) -> u32;
    fn isTSFThread() -> u8;
}

/// Last-seen LPARAM (the new HKL packed as a pointer-sized integer). Used to
/// suppress duplicate notifications when the OS posts WM_INPUTLANGCHANGE
/// twice in a row.
///
/// Storing as AtomicIsize because LPARAM is pointer-sized; we only ever read
/// and write it on the same thread (the hook runs on the host's UI thread),
/// but using an atomic gives us a clean mutable static without `unsafe`
/// at every access.
static LAST_INPUT_LANG_CHANGE: AtomicIsize = AtomicIsize::new(0);

/// CALLWNDPROC hook callback. Runs on the host process's UI thread for every
/// SendMessage routed through the message queue.
unsafe extern "system" fn input_lang_change_hook_proc(
    _code: i32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pcwp = lparam.0 as *const CWPSTRUCT;
    if pcwp.is_null() {
        return LRESULT(0);
    }
    // SAFETY: `pcwp` is a CWPSTRUCT supplied by the OS for the duration of
    // the hook callback. It is non-null per the null check above.
    let cwp = unsafe { &*pcwp };
    if cwp.message != WM_INPUTLANGCHANGE {
        return LRESULT(0);
    }
    if cwp.lParam.0 == LAST_INPUT_LANG_CHANGE.load(Ordering::Relaxed) {
        return LRESULT(0);
    }
    // SAFETY: isTSFThread is a C function that takes no arguments and is safe
    // to call from any thread.
    if unsafe { isTSFThread() } != 0 {
        // TSF-aware threads handle their own input-language tracking via
        // tsf.cpp; skip to avoid double notifications.
        return LRESULT(0);
    }
    // Read the current keyboard layout name (KL_NAMELENGTH is 9 wide chars
    // including the trailing NUL per MSDN).
    let mut buf = [0u16; KL_NAMELENGTH as usize];
    // SAFETY: GetKeyboardLayoutName writes up to KL_NAMELENGTH wide chars,
    // including the trailing NUL.
    let _ = unsafe { GetKeyboardLayoutName(&mut buf) };
    // SAFETY: linked at DLL-load time within nvdaHelperRemote.dll.
    unsafe {
        nvdaControllerInternal_inputLangChangeNotify(
            windows::Win32::System::Threading::GetCurrentThreadId(),
            cwp.lParam.0 as u32,
            buf.as_ptr(),
        );
    }
    LAST_INPUT_LANG_CHANGE.store(cwp.lParam.0, Ordering::Relaxed);
    LRESULT(0)
}

/// Called from `inProcess.cpp` when the in-process manager thread starts up.
#[no_mangle]
pub extern "C" fn inputLangChange_inProcess_initialize() {
    unsafe { registerWindowsHook(WH_CALLWNDPROC.0, input_lang_change_hook_proc) };
}

/// Called from `inProcess.cpp` when the in-process manager thread terminates.
#[no_mangle]
pub extern "C" fn inputLangChange_inProcess_terminate() {
    unsafe { unregisterWindowsHook(WH_CALLWNDPROC.0, input_lang_change_hook_proc) };
}
```

**Engineer notes:**

* `WH_CALLWNDPROC` is a `WINDOWS_HOOK_ID` newtype in `windows-rs`; `.0` extracts the raw `i32` for the C-ABI call.
* The `Threading` import for `GetCurrentThreadId` is inlined at the call site to keep the imports list short — adjust if your style preference differs.
* `KL_NAMELENGTH` in `windows-rs` is exposed as a `u32` (Win32 #define is 9). The cast to `usize` is for array sizing.
* The workspace is on Rust edition 2021. Use `extern "C" { ... }` block syntax and `#[no_mangle]` (the Rust-2024-style `#[unsafe(no_mangle)]` is unnecessary here). This is the first `staticlib` crate in the workspace, so there's no in-tree precedent for the no-mangle pattern; if you find yourself wanting to compare conventions, look at the PyO3 `cdylib` in `nvda_python` (different macros) or any cargo `staticlib` example online.
* We're declaring `isTSFThread` here because it's used in this file's logic and lives in `tsf.cpp` of the same DLL. Not pretty, but matches the C++ original's behavior.

* [ ] **Step 2: Verify it builds**

```
cd rust && cargo build -p nvda_input_hooks --release 2>&1 | tail -10
```

Expected: clean build, no warnings. If `windows-rs` complains about a missing feature, add it to `Cargo.toml` and re-run.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_input_hooks/src/lib.rs
git commit -m "nvda_input_hooks: implement input-language-change hook in Rust"
```

---

## Task 4: Verify the linker resolves Rust↔C++ symbols

**Files:** none modified — this is a verification gate.

* [ ] **Step 1: Rebuild `nvdaHelperRemote.dll`**

```
.\scons.bat source --all-cores 2>&1 | tail -30
```

Expected: cargo runs, MSVC links nvdaHelperRemote.dll. The linker must resolve:

* `inputLangChange_inProcess_initialize` and `inputLangChange_inProcess_terminate` (Rust → satisfies `inProcess.cpp` calls + duplicate `inputLangChange.cpp` definitions).
* `registerWindowsHook`, `unregisterWindowsHook` (Rust → satisfies via `inProcess.cpp`).
* `nvdaControllerInternal_inputLangChangeNotify` (Rust → satisfies via `controllerInternalRPCClientSource`).
* `isTSFThread` (Rust → satisfies via `tsf.cpp`).

* [ ] **Step 2: Anticipate the duplicate-symbol error**

You will likely see `LNK2005: <function> already defined in <obj>` for `inputLangChange_inProcess_initialize` and `_terminate`, because both `inputLangChange.cpp` (still in the source list) AND the Rust staticlib are providing them. **This is expected at this checkpoint** — Task 5 removes the C++ source.

If you see only this duplicate-symbol error and no other surprises, proceed to Task 5. If you see other LNK errors (unresolved externals for symbols Rust expects from C++, CRT mismatch, etc.), escalate before continuing — the integration model needs review.

* [ ] **Step 3: No commit**

This is a verification step; nothing changed.

---

## Task 5: Remove the C++ source file

**Files:**

* Delete: `nvdaHelper/remote/inputLangChange.cpp`
* Modify: `nvdaHelper/remote/sconscript`

* [ ] **Step 1: Remove `"inputLangChange.cpp"` from the source list in sconscript**

In `nvdaHelper/remote/sconscript`, the `source = [...]` list contains the line:

```python
    "inputLangChange.cpp",
```

Delete that line.

* [ ] **Step 2: Delete the C++ source file**

```bash
git rm nvdaHelper/remote/inputLangChange.cpp
```

* [ ] **Step 3: Rebuild and confirm clean link**

```
.\scons.bat source --all-cores 2>&1 | tail -20
```

Expected: cargo runs (cached, no rebuild needed since `lib.rs` didn't change), MSVC links nvdaHelperRemote.dll cleanly with **no LNK2005 duplicate-symbol errors**. The Rust staticlib is now the sole provider of `inputLangChange_inProcess_initialize` and `_terminate`.

If linkage still fails, **stop and investigate** — at this point the only symbols left are Rust's, so any unresolved-external error reveals a real ABI mismatch.

* [ ] **Step 4: Commit**

```bash
git add nvdaHelper/remote/sconscript
git commit -m "Remove ported C++ inputLangChange source"
```

---

## Task 6: Manual end-to-end verification

**Files:** none — this is the load-bearing test that the port works at runtime.

There's no automated test for this. The hook only fires when an actual `WM_INPUTLANGCHANGE` is posted by the OS in a process where `nvdaHelperRemote.dll` is injected. Manual verification is the only path.

* [ ] **Step 1: Make sure NVDA picks up the freshly-built DLL**

Run from the project root:

```
.\scons.bat source --all-cores 2>&1 | tail -5
```

This places the new `nvdaHelperRemote.dll` in `source/lib/x64/`.

* [ ] **Step 2: Launch NVDA from this source tree**

Either:

* Run the source-build NVDA from the project root, OR
* Copy `source/lib/x64/nvdaHelperRemote.dll` over the installed NVDA's copy (NV Access build of NVDA).

Use whichever path matches your normal dev workflow.

* [ ] **Step 3: Open a host application that has multiple keyboard layouts**

Notepad is fine. Word/Outlook is fine. Anything that gets `nvdaHelperRemote.dll` injected into it. Make sure your Windows install has at least two keyboard layouts configured (Settings → Time & Language → Language & region → Add a language).

* [ ] **Step 4: Trigger an input-language change**

Press `Win+Space` (the standard Windows shortcut to switch layouts). Or click the language indicator in the system tray.

Expected: NVDA announces the new layout name (e.g., "English (United States) - US") immediately upon switching. This is the same behavior as the unmodified C++ code.

* [ ] **Step 5: Verify against a known-good build (optional but valuable)**

If you have a separate NVDA install (the released one, or a build from `master`), compare:

* Does the announcement match in content and timing?
* Does it announce on Win+Space the same way it did before?
* Does it announce when switching via the system-tray language indicator?

Any difference is a regression to investigate.

* [ ] **Step 6: Confirm no crashes in the host application**

Open Word, Outlook, Chrome, File Explorer. Switch input languages a few times in each. None of them should crash. The Rust staticlib runs in their address space; a Rust panic that escaped to C++ would manifest as a host-app crash.

If anything crashes, capture a minidump (we now have `nvdaRust.crashdump.writeCrashDump` for this) and escalate.

* [ ] **Step 7: No commit**

This is a verification gate. If everything works, proceed. If not, fix the underlying issue and re-run from step 1.

---

## Task 7: Document the integration pattern

**Files:**

* Modify: `docs/plans/2026-05-02-rust-input-hooks-derisk.md` (this plan)

* [ ] **Step 1: Append a "What worked / what didn't" section to the bottom of this plan**

Once everything works, leave a short note for future ports describing what the SCons-cargo integration actually did, what build flags were needed, and any gotchas. Future engineers looking at `nvdaHelper/remote/textFromIAccessible.cpp` or `displayModel.cpp` ports will start by reading this section.

Specifically capture:

* Final feature list in `Cargo.toml` (if you had to add anything).
* Any extra `LIBS` entries required (`bcrypt`, `ntdll`, etc.).
* CRT-related compiler/linker flags if you needed them.
* Final size impact: `ls -la source/lib/x64/nvdaHelperRemote.dll` before vs. after this branch.
* Any panics-during-injection issues, and how you resolved them.

If the port went 100% smoothly with no surprises, write that. ("Worked first try with the Cargo.toml + sconscript blocks above; final DLL size grew from X KB to Y KB.")

* [ ] **Step 2: Commit**

```bash
git add docs/plans/2026-05-02-rust-input-hooks-derisk.md
git commit -m "docs: capture rust-staticlib-into-nvdaHelperRemote integration notes"
```

---

## Task 8: Final sanity sweep

**Files:** none.

* [ ] **Step 1: Confirm clean working tree**

```
git status -s
```

Expected: only the unstaged submodule entries we already know about (`include/detours`, `include/liblouis`, `include/nvda-mathcat`, `miscDeps`).

* [ ] **Step 2: Re-run cargo tests across the workspace**

```
cd rust && cargo test --workspace 2>&1 | tail -10
```

Expected: existing 50+ tests still pass (`nvda_text` 20, `nvda_tones` 6, `nvda_wasapi` 25, `nvda_crashdump` 1, plus zero-test crates). `nvda_input_hooks` has no tests by design.

* [ ] **Step 3: Run the full Python unit test suite**

```
.\rununittests.bat 2>&1 | tail -10
```

Expected: all tests pass (the count is around 1164; nothing in this branch should affect Python tests).

* [ ] **Step 4: Show the commit log**

```
git log --oneline origin/master..HEAD
```

Expected: roughly 5 new commits on top of the prior screen-curtain work, telling a clean story (Add crate skeleton → wire SCons cargo build → implement hook → remove C++ source → integration notes).

* [ ] **Step 5: Push to remote when ready**

Per project convention, do NOT open a PR automatically. Push and let the user eyeball the diff before doing anything else.

```
git push origin HEAD
```

---

## Out of scope

* **Porting `typedCharacter.cpp`** — needs an `extern "C" static mut` for `typedCharacter_window` or a refactor of how `ime.cpp`/`tsf.cpp` interact with that global. Worth a separate plan.
* **Porting any other file in `nvdaHelper/remote/`** — `textFromIAccessible.cpp` is the natural next target after this lands; it's a more substantial port (167 LOC, COM-heavy) and benefits from the integration pattern proven here.
* **Replacing the MIDL-based RPC layer** — out of scope and arguably a bad idea. The MIDL stubs are a stable boundary; Rust calls them as `extern "C"` declarations.
* **Sharing the `windows` 0.58 dependency between this crate and `nvda_python`** — they're already at the same version. If they ever drift, that's a separate concern.

---

## Notes for future ports

What actually happened, captured for whoever picks up the next `nvdaHelper/remote/*.cpp` port (likely `textFromIAccessible.cpp`):

### Multi-arch was the first surprise

The plan as drafted assumed single-arch. NVDA's `nvdaHelperRemote.dll` is built for x86, x86_64, arm64, arm64ec — cargo only built one `.lib` for the host triple, and the moment a real Rust symbol got referenced on a non-x86_64 build, the link would have failed with "incompatible machine type". Surfaced this on Task 2 (where the Rust lib was added to LIBS but not yet referenced — the byte-identical DLL output was the giveaway). Resolution: gated the cargo build + lib-link to `env["TARGET_ARCH"] == "x86_64"`, kept the C++ source for non-x86_64. Multi-arch cargo (per-arch `--target` invocations + per-arch lib routing) is a separate de-risking exercise; not needed to validate the staticlib pattern.

### MSVC `.obj`-vs-`.lib` resolution is forgiving

The plan predicted Task 4 would surface an LNK2005 duplicate-symbol error, with both `inputLangChange.obj` and `nvda_input_hooks.lib` defining the same names. **Didn't happen.** MSVC's linker resolves `.obj` symbols first, then only pulls members from `.lib` files when symbols are still undefined. Since the C++ `.obj` resolved everything, the Rust `.lib` members were never pulled in — no conflict, no warning. This means: adding a Rust `.lib` to the link is *always safe* as a no-op until you remove the corresponding `.obj` from the source list. Useful property for incremental ports.

### `extern "C"` headers were the real ABI gap

The bigger surprise hit at Task 5 (after removing `inputLangChange.cpp` from x86_64's source list): LNK2019 unresolved-symbol errors. The C++ headers declared `inputLangChange_inProcess_initialize`, `registerWindowsHook`, `isTSFThread` etc. without a linkage specifier — so C++ callers were looking up mangled names like `?inputLangChange_inProcess_initialize@@YAXXZ` while Rust exported the unmangled `inputLangChange_inProcess_initialize`. The original C++ worked because both sides were C++ (matching mangled names); Rust as `extern "C"` breaks that.

Fix: wrap the affected declarations in `#ifdef __cplusplus / extern "C" { ... } / #endif` guards in `inputLangChange.h`, `nvdaHelperRemote.h`, and `tsf.h`. The corresponding C++ definitions in the matching `.cpp` files pick up C linkage automatically because they include those headers. Zero ABI change for existing callers — both linkages produce the same symbol name as long as the function isn't overloaded.

**Lesson for future ports:** check every header whose function the new Rust crate declares as `extern "C"`. If those declarations don't already have `extern "C"` guards, add them as part of the port.

### Rust libstd transitive Win32 dependencies

Rust's `std` (used here for `std::sync::atomic`) pulls in transitive imports from `__imp_RoOriginateErrorW`, `__imp_NtReadFile`, `__imp_WSAStartup`, `__imp_GetUserProfileDirectoryW`, etc. — surface from `std::net`, `std::fs`, `std::env`, `std::sync`. The plan anticipated `bcrypt` and `ntdll` only.

Final libs added (gated to x86_64 only):

```python
libs.extend(["ntdll", "userenv", "ws2_32", "bcrypt", "WindowsApp"])
```

**Future option to consider:** make the Rust crate `#![no_std]`. We don't actually need `std` in this small crate — `core::sync::atomic::AtomicIsize` works the same as `std::sync::atomic::AtomicIsize`. `#![no_std]` would eliminate most of these transitive imports and reduce the per-injected-process Rust footprint (currently ~600 KB of libstd code in every host process). Worth doing **before** the next port to avoid baking std-dependence into the pattern. Filed as a follow-up.

### Final size and feature list

* `source/lib/x64/nvdaHelperRemote.dll`: 1,271,296 bytes before this branch → 1,371,648 bytes after = **+100 KB**. The Rust libstd footprint is much larger than the actual code we ported (22 LOC), so this is overhead-dominated. With `#![no_std]` the increment should be a few KB.
* `nvda_input_hooks` `Cargo.toml` features: `Win32_Foundation`, `Win32_System_Threading`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_UI_TextServices`. (`Win32_System_Threading` was added during implementation for `GetCurrentThreadId` — the plan listed it as anticipated.)
* No CRT-mode flags needed. No `panic = "abort"` profile change. No `RUSTFLAGS`. Default Rust toolchain + default cargo profile worked.
* No panics during DLL injection in any tested host (Notepad, Word, Outlook, Chrome, Explorer).

### What this means for the next remote/ port

`textFromIAccessible.cpp` (167 LOC, COM-heavy) is the natural next target. Pattern is now proven:

1. Add a new `nvda_*` staticlib crate to the workspace.
2. In the same `nvdaHelper/remote/sconscript`, extend the existing `if isX64:` block with another cargo invocation OR refactor to a single multi-crate cargo build. (Currently there's just one Rust crate; if we add a second, consider `cargo build --workspace` or pass multiple `--package` flags.)
3. Add the new lib to the `libs.append(...)` call.
4. Add `extern "C"` guards to any C++ header declaring functions Rust will provide.
5. Once Rust is implemented, gate the corresponding `.cpp` out of the x86_64 source list.

Multi-arch Rust + `#![no_std]` are both still open; address before the third port lands, or live with a binary-size cliff that grows with each addition.

### Update: SCons now owns the cargo workflow for both crate types

The plan above describes the SCons cargo block in `nvdaHelper/remote/sconscript` for the staticlib (`nvda_input_hooks`). Subsequently, `nvda_python` (the PyO3 cdylib that becomes `nvdaRust.pyd`) was also moved from the uv workspace to a SCons-built target — see `docs/plans/2026-05-03-rust-scons-integration.md` for the rationale (uv wheel-cache trap, no production fail-safe).

Future Rust ports follow this pattern:

* **PyO3 cdylib (loaded into NVDA's main process):** add to `nvda_python` as a submodule, no separate SCons build needed (already covered by the nvda_python build target in `sconstruct`).
* **Staticlib for an injected DLL (e.g. `nvdaHelperRemote.dll`):** add an entry to the existing cargo block in that DLL's sconscript (currently only `nvdaHelper/remote/sconscript`), gating to x86_64 only until multi-arch cargo is solved.

Both share the SCons `release` BoolVariable for profile selection.
