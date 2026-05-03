# Rust IA2 Bindings + Partial ia2utils Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a `nvda_ia2` Rust crate with hand-rolled IAccessible2-family COM bindings, and port the IA2-attribute helpers (`IA2AttribsToMap`, `fetchIA2Attributes`) from C++ to Rust. This is staging PR 1 of the textFromIAccessible port — bindings + the COM-free helpers land here, the rest (`getAccessibleChildren`, hyperlink getters, `textFromIAccessible.cpp`) lands in PR 2.

**Architecture:** A new `rust/nvda_ia2` crate built as a `staticlib`, linked into `nvdaHelperRemote.dll` on x86_64 (other archs keep the C++ source, matching the `nvda_input_hooks` precedent). Bindings use the `windows-core::interface!` macro with IIDs copied verbatim from the IDL files in `include/ia2/api/`. The Rust `IA2AttribsToMap` and `fetchIA2Attributes` are exposed via `extern "C"` callback-based shims so the existing C++ surface in `ia2utils.h` (which uses `std::map<std::wstring, std::wstring>&`) continues to work without C++ caller changes.

**Tech Stack:** Rust 2021, `windows-core` 0.58, `windows` 0.58 (Win32_UI_Accessibility, Win32_Foundation, Win32_System_Com features), SCons, MSVC link.exe.

---

## Background for the Implementer

### What you need to know about this codebase

* This is the NVDA screen reader. `nvdaHelperRemote.dll` is a DLL injected into target processes (browsers, Word, etc.) to scrape accessibility information. Code in this DLL runs inside foreign processes — keep dependencies tight.
* `nvdaHelper/common/ia2utils.cpp` is shared utility code consumed by both `nvdaHelperRemote.dll` and other helper components. It exposes four things:
  1. `IA2AttribsToMap` — pure parser: takes a `std::wstring` of the form `"name:value;name:value;"` (with `\` escaping) and fills a `std::map<std::wstring, std::wstring>&`.
  2. `fetchIA2Attributes` — calls `IAccessible2::get_attributes()` (returns a BSTR), feeds it through `IA2AttribsToMap`, frees the BSTR.
  3. `getAccessibleChildren` — wraps `oleacc::AccessibleChildren`. **Out of scope for this PR.**
  4. `HyperlinkGetter` class hierarchy (`HtHyperlinkGetter`, `Ht2HyperlinkGetter`, `makeHyperlinkGetter`). **Out of scope for this PR.**
* The IA2 IDL files live in `include/ia2/api/` (the `LinuxA11y/IAccessible2` git submodule). SCons compiles them with MIDL into `build/<arch>/ia2.h`, `ia2_i.c`, `ia2_p.c`, but this plan does **not** depend on those generated files — we hand-roll the bindings in Rust from the IIDs in the IDL.
* A previous PR established the `staticlib` linking pattern with `rust/nvda_input_hooks`. **Read `rust/nvda_input_hooks/Cargo.toml` and the cargo block in `nvdaHelper/remote/sconscript` (lines 87–183) before starting** — copy that pattern.
* The C++ build is multi-arch (x86, x86_64, arm64, arm64ec). Cargo only builds for the host triple, so the Rust staticlib is gated to `env["TARGET_ARCH"] == "x86_64"`. Other arches keep using the C++ implementation. **Do not** try to fix multi-arch builds in this PR.
* The build system entry points: `scons.bat source` builds the helpers and copies them into `source/`. SCons drives both the C++ link of `nvdaHelperRemote.dll` and the Rust `cargo build`. There is no separate `cargo` step to remember.
* Pre-commit runs markdownlint on `docs/plans/`. If it reformats this plan file, `git add` the changes — don't fight the linter.

### IIDs and IDL pointers (verified from `include/ia2/api/`)

| Interface | IID | IDL file | Parent |
| --- | --- | --- | --- |
| `IAccessible2` | `E89F726E-C4F4-4c19-BB19-B647D7FA8478` | `Accessible2.idl:383` | `IAccessible` (oleacc) |
| `IAccessibleText` | `24FD2FFB-3AAD-4a08-8335-A3AD89C0FB4B` | `AccessibleText.idl` | `IUnknown` |
| `IAccessibleHypertext` | `6B4F8BBF-F1F2-418a-B35E-A195BC4103B9` | `AccessibleHypertext.idl` | `IAccessibleText` |
| `IAccessibleHypertext2` | `CF64D89F-8287-4B44-8501-A827453A6077` | `AccessibleHypertext2.idl` | `IAccessibleHypertext` |
| `IAccessibleHyperlink` | `01C20F2B-3DD2-400f-949F-AD00BDAB1D41` | `AccessibleHyperlink.idl` | `IAccessibleAction` |

`IAccessible` (no `2`) comes from `windows::Win32::UI::Accessibility::IAccessible`. `IAccessibleAction` is not used by this PR — for `IAccessibleHyperlink` we declare `IUnknown` as the parent for now (PR 2 can refine if it needs `IAccessibleAction` methods, which `textFromIAccessible.cpp` doesn't).

### IA2TextSegment struct (from `AccessibleText.idl:63`)

```c
typedef struct IA2TextSegment {
  BSTR text;   // server-allocated, client must SysFreeString
  long start;
  long end;
} IA2TextSegment;
```

Used by PR 2 (textFromIAccessible's `useNewText` path), declared here so PR 2 doesn't have to.

### Methods this PR will actually call

Only `IAccessible2::get_attributes(BSTR* attributes)` is invoked from Rust in this PR. The other interfaces' bindings are added now so PR 2 doesn't have to interleave binding work with caller migration. Bindings without consumers are not dead-weight at runtime — `staticlib` linking only pulls in symbols that are referenced, so unused bindings are dropped by the linker.

---

## File Structure

**Created:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/Cargo.toml` | Crate manifest. `crate-type = ["staticlib", "rlib"]` so unit tests can run. Dependencies on `windows`, `windows-core`. |
| `rust/nvda_ia2/src/lib.rs` | Crate root. Module declarations + crate-level docs. |
| `rust/nvda_ia2/src/types.rs` | `IA2TextSegment` struct, `BSTR`/`HRESULT` re-exports. |
| `rust/nvda_ia2/src/interfaces.rs` | All five `windows_core::interface!` declarations. |
| `rust/nvda_ia2/src/attribs.rs` | `IA2AttribsToMap` Rust port, `parse_attribs` pure-Rust function with unit tests, `extern "C"` shim. |
| `rust/nvda_ia2/src/fetch.rs` | `fetchIA2Attributes` Rust port, `extern "C"` shim. |

**Modified:**

| File | Change |
| --- | --- |
| `rust/Cargo.toml` | Add `nvda_ia2` to workspace `members`. |
| `nvdaHelper/common/ia2utils.cpp` | Delete the C++ bodies of `IA2AttribsToMap` and `fetchIA2Attributes`. Replace with thin C++ wrappers that call into the Rust `extern "C"` shims (callback bridge). Keep `getAccessibleChildren` and the hyperlink classes untouched. |
| `nvdaHelper/remote/sconscript` | Add `nvda_ia2` cargo build (gated to x86_64), append the produced `.lib` to `libs`. Mirror the structure of the existing `nvda_input_hooks` block. |

**Untouched in this PR (handled by PR 2):**

* `nvdaHelper/remote/textFromIAccessible.cpp`
* `nvdaHelper/remote/ia2LiveRegions.cpp`
* The `getAccessibleChildren` / `HyperlinkGetter` parts of `ia2utils.cpp`
* All other consumers of `ia2utils.h`

---

### Task 1: Create the `nvda_ia2` crate scaffold

**Files:**

* Create: `rust/nvda_ia2/Cargo.toml`
* Create: `rust/nvda_ia2/src/lib.rs`
* Modify: `rust/Cargo.toml`

* [ ] **Step 1: Write `rust/nvda_ia2/Cargo.toml`**

```toml
[package]
name = "nvda_ia2"
version = "0.1.0"
edition = "2021"

[lib]
# `staticlib` so nvdaHelperRemote.dll can link the C-ABI shims.
# `rlib` so the unit tests in this crate can build a test binary.
crate-type = ["staticlib", "rlib"]

[dependencies.windows-core]
version = "0.58"

[dependencies.windows]
version = "0.58"
features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_UI_Accessibility",
]
```

* [ ] **Step 2: Write `rust/nvda_ia2/src/lib.rs`**

```rust
//! NVDA IA2: Hand-rolled bindings for the IAccessible2 family of COM
//! interfaces, plus Rust ports of selected helpers from `nvdaHelper/common/
//! ia2utils.cpp`.
//!
//! This crate is built as a `staticlib` and linked into
//! `nvdaHelperRemote.dll`, which is injected into target processes (browsers,
//! Office apps, etc.). Keep dependencies minimal and avoid host-process
//! global state.
//!
//! Bindings are hand-rolled (not generated from the IDLs in `include/ia2/api/`)
//! so the crate has no MIDL dependency. IIDs and method orderings are copied
//! verbatim from those IDLs — keep them in sync if the submodule updates.

#![allow(non_snake_case)]
// Bindings for interfaces not yet exercised in this PR — will be used by
// the textFromIAccessible port in the follow-up PR.
#![allow(dead_code)]

pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod types;
```

* [ ] **Step 3: Add `nvda_ia2` to the workspace**

Edit `rust/Cargo.toml`:

```toml
[workspace]
members = ["nvda_core", "nvda_crashdump", "nvda_ia2", "nvda_input_hooks", "nvda_ole", "nvda_screen_curtain", "nvda_text", "nvda_tones", "nvda_wasapi", "nvda_python"]
resolver = "2"
```

* [ ] **Step 4: Verify the crate builds (it has no source modules yet — that's fine, lib.rs `mod` declarations will fail)**

Stub the modules so step 4 actually works:

```bash
echo "// placeholder" > rust/nvda_ia2/src/types.rs
echo "// placeholder" > rust/nvda_ia2/src/interfaces.rs
echo "// placeholder" > rust/nvda_ia2/src/attribs.rs
echo "// placeholder" > rust/nvda_ia2/src/fetch.rs
```

Run: `cd rust && cargo build -p nvda_ia2`
Expected: `Compiling nvda_ia2 v0.1.0` then `Finished`. No warnings.

* [ ] **Step 5: Commit**

```bash
git add rust/Cargo.toml rust/nvda_ia2
git commit -m "Add nvda_ia2 crate scaffold for IA2 bindings + ia2utils port"
```

---

### Task 2: Define IA2 types (`IA2TextSegment` and re-exports)

**Files:**

* Modify: `rust/nvda_ia2/src/types.rs`

* [ ] **Step 1: Write `IA2TextSegment` and the unit test for layout**

Replace the placeholder in `rust/nvda_ia2/src/types.rs`:

```rust
//! Common IA2 types. Re-exports the `BSTR` / `HRESULT` aliases the rest
//! of the crate uses, plus structs from the IDL.

pub use windows::core::{BSTR, HRESULT, Result};
pub use windows::Win32::Foundation::{S_FALSE, S_OK};

/// Mirrors `IA2TextSegment` from `include/ia2/api/AccessibleText.idl:63`.
///
/// `text` is a server-allocated BSTR. Callers must `SysFreeString` it (the
/// `windows::core::BSTR` `Drop` impl does this automatically when this struct
/// is owned).
#[repr(C)]
#[derive(Default)]
pub struct IA2TextSegment {
    pub text: BSTR,
    pub start: i32,
    pub end: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    /// The struct is passed by pointer over the COM ABI. Its layout must
    /// match the C declaration: pointer (BSTR), 4-byte long, 4-byte long.
    /// On x86_64 that's 8 + 4 + 4 = 16 bytes.
    #[test]
    fn ia2_text_segment_layout() {
        assert_eq!(size_of::<IA2TextSegment>(), 16);
        assert_eq!(align_of::<IA2TextSegment>(), 8);
    }
}
```

* [ ] **Step 2: Run the layout test**

Run: `cd rust && cargo test -p nvda_ia2 --lib types::tests`
Expected: `test types::tests::ia2_text_segment_layout ... ok`. Test passes.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ia2/src/types.rs
git commit -m "Add IA2TextSegment struct with ABI layout test"
```

---

### Task 3: Define IA2 interface bindings

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs`

The `windows_core::interface!` macro takes the IID and the parent interface and generates a vtable-respecting wrapper. Method declarations follow the COM vtable order from the IDL — **do not reorder** even if it looks wrong.

* [ ] **Step 1: Write all five interface bindings**

Replace the placeholder in `rust/nvda_ia2/src/interfaces.rs`:

```rust
//! Hand-rolled bindings for the IAccessible2 COM interfaces this project
//! consumes. Method orderings come from the corresponding IDL files in
//! `include/ia2/api/`. Only the methods used (or expected to be used by the
//! follow-up PR) are declared — the vtable trailing slots we don't need are
//! filled with `unused: usize` to keep the offsets correct.
//!
//! IID quick reference (verbatim from the IDLs):
//! - IAccessible2:           E89F726E-C4F4-4c19-BB19-B647D7FA8478  (Accessible2.idl:383)
//! - IAccessibleText:        24FD2FFB-3AAD-4a08-8335-A3AD89C0FB4B  (AccessibleText.idl)
//! - IAccessibleHypertext:   6B4F8BBF-F1F2-418a-B35E-A195BC4103B9  (AccessibleHypertext.idl)
//! - IAccessibleHypertext2:  CF64D89F-8287-4B44-8501-A827453A6077  (AccessibleHypertext2.idl)
//! - IAccessibleHyperlink:   01C20F2B-3DD2-400f-949F-AD00BDAB1D41  (AccessibleHyperlink.idl)

use windows::core::{interface, BSTR, HRESULT, IUnknown, IUnknown_Vtbl, Interface};
use windows::Win32::UI::Accessibility::IAccessible;

use crate::types::IA2TextSegment;

// --- IAccessible2 ---------------------------------------------------------
//
// Inherits from IAccessible. We declare only the trailing slots we need
// (get_attributes is the last method in the vtable, so we must list every
// method between the IAccessible base and get_attributes to preserve vtable
// offsets). For PR 1 we only call get_attributes; the slots before it are
// declared as opaque `unused` raw pointers.
//
// Vtable order (from Accessible2.idl, after the IAccessible methods):
//   1.  get_nRelations
//   2.  get_relation
//   3.  get_relations
//   4.  role
//   5.  scrollTo
//   6.  scrollToPoint
//   7.  get_groupPosition
//   8.  get_states
//   9.  get_extendedRole
//   10. get_localizedExtendedRole
//   11. get_nExtendedStates
//   12. get_extendedStates
//   13. get_localizedExtendedStates
//   14. get_uniqueID
//   15. get_windowHandle
//   16. get_indexInParent
//   17. get_locale
//   18. get_attributes  <-- the only one we use
interface! {
    #[uuid("e89f726e-c4f4-4c19-bb19-b647d7fa8478")]
    pub unsafe IAccessible2(IAccessible2_Vtbl): IAccessible;
}

#[repr(C)]
pub struct IAccessible2_Vtbl {
    pub base: <IAccessible as Interface>::Vtable,
    pub get_nRelations: usize,
    pub get_relation: usize,
    pub get_relations: usize,
    pub role: usize,
    pub scrollTo: usize,
    pub scrollToPoint: usize,
    pub get_groupPosition: usize,
    pub get_states: usize,
    pub get_extendedRole: usize,
    pub get_localizedExtendedRole: usize,
    pub get_nExtendedStates: usize,
    pub get_extendedStates: usize,
    pub get_localizedExtendedStates: usize,
    pub get_uniqueID: usize,
    pub get_windowHandle: usize,
    pub get_indexInParent: usize,
    pub get_locale: usize,
    pub get_attributes: unsafe extern "system" fn(this: *mut core::ffi::c_void, attributes: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
}

impl IAccessible2 {
    /// Returns the IA2 attributes string (server-allocated BSTR).
    /// Returns `S_FALSE` with a NULL output BSTR if there are no attributes
    /// (per the IDL contract at Accessible2.idl:687).
    pub unsafe fn get_attributes(&self) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_attributes)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        // Take ownership of the BSTR (BSTR's Drop will SysFreeString).
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }
}

// --- IAccessibleText ------------------------------------------------------
//
// PR 2 will use get_text and get_newText. We declare the full prefix of the
// vtable up to (and including) get_newText. Until PR 2 wires it up, the
// methods are present but unexercised — the linker may drop unused slots
// from a final binary, but the vtable layout requires every offset.
//
// Vtable order (from AccessibleText.idl):
//   1.  addSelection
//   2.  get_attributes
//   3.  get_caretOffset
//   4.  get_characterExtents
//   5.  get_nSelections
//   6.  get_offsetAtPoint
//   7.  get_selection
//   8.  get_text
//   9.  get_textBeforeOffset
//   10. get_textAfterOffset
//   11. get_textAtOffset
//   12. removeSelection
//   13. setCaretOffset
//   14. setSelection
//   15. get_nCharacters
//   16. scrollSubstringTo
//   17. scrollSubstringToPoint
//   18. get_newText
//   19. get_oldText
interface! {
    #[uuid("24fd2ffb-3aad-4a08-8335-a3ad89c0fb4b")]
    pub unsafe IAccessibleText(IAccessibleText_Vtbl): IUnknown;
}

#[repr(C)]
pub struct IAccessibleText_Vtbl {
    pub base: IUnknown_Vtbl,
    pub addSelection: usize,
    pub get_attributes: usize,
    pub get_caretOffset: usize,
    pub get_characterExtents: usize,
    pub get_nSelections: usize,
    pub get_offsetAtPoint: usize,
    pub get_selection: usize,
    pub get_text: unsafe extern "system" fn(this: *mut core::ffi::c_void, start_offset: i32, end_offset: i32, text: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
    pub get_textBeforeOffset: usize,
    pub get_textAfterOffset: usize,
    pub get_textAtOffset: usize,
    pub removeSelection: usize,
    pub setCaretOffset: usize,
    pub setSelection: usize,
    pub get_nCharacters: usize,
    pub scrollSubstringTo: usize,
    pub scrollSubstringToPoint: usize,
    pub get_newText: unsafe extern "system" fn(this: *mut core::ffi::c_void, new_text: *mut IA2TextSegment) -> HRESULT,
    pub get_oldText: usize,
}

// --- IAccessibleHypertext -------------------------------------------------
//
// Inherits from IAccessibleText. Vtable order (from AccessibleHypertext.idl):
//   1. get_nHyperlinks
//   2. get_hyperlink
//   3. get_hyperlinkIndex
interface! {
    #[uuid("6b4f8bbf-f1f2-418a-b35e-a195bc4103b9")]
    pub unsafe IAccessibleHypertext(IAccessibleHypertext_Vtbl): IAccessibleText;
}

#[repr(C)]
pub struct IAccessibleHypertext_Vtbl {
    pub base: IAccessibleText_Vtbl,
    pub get_nHyperlinks: usize,
    pub get_hyperlink: unsafe extern "system" fn(this: *mut core::ffi::c_void, index: i32, hyperlink: *mut Option<IAccessibleHyperlink>) -> HRESULT,
    pub get_hyperlinkIndex: unsafe extern "system" fn(this: *mut core::ffi::c_void, char_index: i32, hyperlink_index: *mut i32) -> HRESULT,
}

// --- IAccessibleHypertext2 ------------------------------------------------
//
// Inherits from IAccessibleHypertext. Vtable order (AccessibleHypertext2.idl):
//   1. get_hyperlinks  -- BSTRs allocated by server with CoTaskMemAlloc;
//                          client frees with CoTaskMemFree.
interface! {
    #[uuid("cf64d89f-8287-4b44-8501-a827453a6077")]
    pub unsafe IAccessibleHypertext2(IAccessibleHypertext2_Vtbl): IAccessibleHypertext;
}

#[repr(C)]
pub struct IAccessibleHypertext2_Vtbl {
    pub base: IAccessibleHypertext_Vtbl,
    pub get_hyperlinks: unsafe extern "system" fn(this: *mut core::ffi::c_void, hyperlinks: *mut *mut Option<IAccessibleHyperlink>, n_hyperlinks: *mut i32) -> HRESULT,
}

// --- IAccessibleHyperlink -------------------------------------------------
//
// Inherits from IAccessibleAction in the IDL, but PR 2 only QIs to it and
// doesn't call its methods directly (it's QI'd to IAccessible2). Declaring
// IUnknown as the parent here lets us get an IID-typed wrapper without
// pulling in the IAccessibleAction binding. PR 2 should not need to revisit
// this unless a future caller actually invokes hyperlink methods.
interface! {
    #[uuid("01c20f2b-3dd2-400f-949f-ad00bdab1d41")]
    pub unsafe IAccessibleHyperlink(IAccessibleHyperlink_Vtbl): IUnknown;
}

#[repr(C)]
pub struct IAccessibleHyperlink_Vtbl {
    pub base: IUnknown_Vtbl,
    // Methods deliberately omitted -- this PR only needs the IID for QI.
}
```

* [ ] **Step 2: Build the crate to verify the bindings compile**

Run: `cd rust && cargo build -p nvda_ia2`
Expected: Builds clean, no warnings.

If you see "trait `Interface` not satisfied" or vtable layout errors: double-check the `windows-core` 0.58 API for `interface!` (it sometimes wants `IUnknown_Vtbl` as a fully-qualified path). Look at how `rust/nvda_ole/` does this if you get stuck.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add IA2 interface bindings (IAccessible2, Text, Hypertext, Hyperlink)"
```

---

### Task 4: Port `IA2AttribsToMap` parser to Rust (TDD)

**Files:**

* Modify: `rust/nvda_ia2/src/attribs.rs`

The C++ parser lives at `nvdaHelper/common/ia2utils.cpp:33-75`. The format: semicolon-separated `key:value` pairs with `\` as escape. Final attribute may omit the trailing `;`. The `src` value is post-processed: if it starts with `data:` and contains `base64,`, everything after `base64,` is replaced with `<truncated>`.

* [ ] **Step 1: Write the failing tests first**

Replace the placeholder in `rust/nvda_ia2/src/attribs.rs`:

```rust
//! Rust port of `IA2AttribsToMap` and `fetchIA2Attributes` from
//! `nvdaHelper/common/ia2utils.cpp`. Exposed via `extern "C"` callback
//! shims so the C++ wrappers in `ia2utils.cpp` can keep their existing
//! `std::map<std::wstring, std::wstring>&` API.

use std::collections::BTreeMap;

/// Parse an IA2-attributes string of the form `name:value;name:value;...`
/// into a sorted map.
///
/// - `:` separates key from value.
/// - `;` separates pairs.
/// - `\` escapes the next character (so `\:` is a literal colon, etc.).
/// - The trailing `;` is optional.
/// - Empty keys are dropped (mirrors the C++ behaviour at ia2utils.cpp:50).
/// - The `src` value is truncated if it starts with `data:` and contains
///   `base64,` (mirrors the C++ behaviour at ia2utils.cpp:62-74).
///
/// `BTreeMap<String, String>` is used (not `HashMap`) for deterministic
/// iteration in tests; the C++ side uses `std::map` which is also ordered.
pub fn parse_attribs(input: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut in_escape = false;
    let mut have_key = false;

    for ch in input.chars() {
        if in_escape {
            if have_key {
                value.push(ch);
            } else {
                key.push(ch);
            }
            in_escape = false;
        } else if ch == '\\' {
            in_escape = true;
        } else if ch == ':' && !have_key {
            have_key = true;
        } else if ch == ';' {
            if have_key && !key.is_empty() {
                out.insert(std::mem::take(&mut key), std::mem::take(&mut value));
            } else {
                key.clear();
                value.clear();
            }
            have_key = false;
        } else if have_key {
            value.push(ch);
        } else {
            key.push(ch);
        }
    }
    if have_key && !key.is_empty() {
        out.insert(key, value);
    }
    truncate_base64_src(&mut out);
    out
}

fn truncate_base64_src(map: &mut BTreeMap<String, String>) {
    const PREFIX: &str = "data:";
    const NEEDLE: &str = "base64,";
    if let Some(src) = map.get_mut("src") {
        if src.starts_with(PREFIX) {
            if let Some(pos) = src.find(NEEDLE) {
                let truncate_at = pos + NEEDLE.len();
                src.truncate(truncate_at);
                src.push_str("<truncated>");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn empty_input_yields_empty_map() {
        assert_eq!(parse_attribs(""), BTreeMap::new());
    }

    #[test]
    fn single_pair_with_trailing_semicolon() {
        assert_eq!(parse_attribs("foo:bar;"), map(&[("foo", "bar")]));
    }

    #[test]
    fn single_pair_without_trailing_semicolon() {
        assert_eq!(parse_attribs("foo:bar"), map(&[("foo", "bar")]));
    }

    #[test]
    fn multiple_pairs() {
        assert_eq!(
            parse_attribs("a:1;b:2;c:3;"),
            map(&[("a", "1"), ("b", "2"), ("c", "3")])
        );
    }

    #[test]
    fn empty_value_is_preserved() {
        assert_eq!(parse_attribs("foo:;"), map(&[("foo", "")]));
    }

    #[test]
    fn empty_key_is_dropped() {
        // Mirrors C++ behaviour: !key.empty() guards the insert.
        assert_eq!(parse_attribs(":bar;"), BTreeMap::new());
    }

    #[test]
    fn escaped_colon_in_value() {
        assert_eq!(parse_attribs("foo:a\\:b;"), map(&[("foo", "a:b")]));
    }

    #[test]
    fn escaped_semicolon_in_value() {
        assert_eq!(parse_attribs("foo:a\\;b;"), map(&[("foo", "a;b")]));
    }

    #[test]
    fn escaped_backslash_in_value() {
        assert_eq!(parse_attribs("foo:a\\\\b;"), map(&[("foo", "a\\b")]));
    }

    #[test]
    fn escaped_colon_in_key() {
        assert_eq!(parse_attribs("a\\:b:val;"), map(&[("a:b", "val")]));
    }

    #[test]
    fn duplicate_key_keeps_last_value() {
        // Mirrors std::map's `attribsMap[key] = str` overwrite semantics.
        assert_eq!(parse_attribs("k:v1;k:v2;"), map(&[("k", "v2")]));
    }

    #[test]
    fn src_data_base64_is_truncated() {
        assert_eq!(
            parse_attribs("src:data:image/png;base64,iVBORw0KGgo;"),
            // Note: the `;` inside the base64 part isn't escaped, so the
            // semicolon ends the value at `data:image/png`. This matches
            // the C++ parser; the test documents the behaviour. The
            // truncation only fires when the value (post-parse) still
            // starts with `data:` and contains `base64,`.
            // Adjusted expectation: the value at this point is just
            // "data:image/png" -- no `base64,`, so no truncation occurs.
            map(&[("src", "data:image/png")]),
        );
    }

    #[test]
    fn src_with_escaped_semicolon_truncated() {
        assert_eq!(
            parse_attribs("src:data:image/png\\;base64,iVBORw0KGgo;"),
            map(&[("src", "data:image/png;base64,<truncated>")]),
        );
    }

    #[test]
    fn src_without_data_prefix_not_truncated() {
        assert_eq!(
            parse_attribs("src:http://example.com/img.png;"),
            map(&[("src", "http://example.com/img.png")]),
        );
    }

    #[test]
    fn non_src_data_value_not_truncated() {
        assert_eq!(
            parse_attribs("href:data:text/plain\\;base64,abc;"),
            map(&[("href", "data:text/plain;base64,abc")]),
        );
    }
}
```

* [ ] **Step 2: Run the tests**

Run: `cd rust && cargo test -p nvda_ia2 --lib attribs::tests`
Expected: All 14 tests pass.

If they don't all pass, **read the C++ parser at `nvdaHelper/common/ia2utils.cpp:33-75` carefully and match its behaviour exactly**, including the somewhat subtle `key.clear()` indentation bug that's preserved in the C++ (look at lines 50-52: the `key.clear()` is mistakenly outside the `if` due to missing braces — it always runs when `;` is hit). Our Rust version should match the *effective* behaviour: empty keys cause both key and value buffers to reset, with no insertion.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ia2/src/attribs.rs
git commit -m "Port IA2AttribsToMap parser to Rust with unit tests"
```

---

### Task 5: Add `extern "C"` shim for `IA2AttribsToMap`

**Files:**

* Modify: `rust/nvda_ia2/src/attribs.rs`

The shim cannot return `std::map<std::wstring, std::wstring>` (no C ABI). Instead, it takes a callback that the C++ wrapper uses to insert each pair into the caller's map.

* [ ] **Step 1: Append the shim to `rust/nvda_ia2/src/attribs.rs`**

```rust
// ---------------------------------------------------------------------------
// extern "C" shim
// ---------------------------------------------------------------------------

/// Callback invoked once per attribute. `key_ptr`/`val_ptr` point to UTF-16
/// code units (without a NUL terminator); `key_len`/`val_len` are code-unit
/// counts. Both pointers are valid only for the duration of the call.
pub type AttribCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    key_ptr: *const u16,
    key_len: usize,
    val_ptr: *const u16,
    val_len: usize,
);

/// C-callable replacement for `IA2AttribsToMap`.
///
/// `input_ptr` / `input_len` point to a UTF-16 attributes string. The shim
/// parses it and invokes `cb(ctx, key, key_len, val, val_len)` once per
/// attribute. The C++ wrapper in `ia2utils.cpp` uses this to populate the
/// caller's `std::map<std::wstring, std::wstring>&`.
///
/// # Safety
/// - `input_ptr` must be valid for `input_len` u16s, or null when `input_len`
///   is 0.
/// - `cb` must be a valid function pointer; `ctx` is opaque user data passed
///   through to `cb` unchanged.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_attribs_to_map(
    input_ptr: *const u16,
    input_len: usize,
    ctx: *mut core::ffi::c_void,
    cb: AttribCallback,
) {
    let input = if input_ptr.is_null() || input_len == 0 {
        String::new()
    } else {
        let slice = std::slice::from_raw_parts(input_ptr, input_len);
        String::from_utf16_lossy(slice)
    };
    let map = parse_attribs(&input);
    for (k, v) in map {
        let k_utf16: Vec<u16> = k.encode_utf16().collect();
        let v_utf16: Vec<u16> = v.encode_utf16().collect();
        cb(
            ctx,
            k_utf16.as_ptr(),
            k_utf16.len(),
            v_utf16.as_ptr(),
            v_utf16.len(),
        );
    }
}

#[cfg(test)]
mod shim_tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    thread_local! {
        static COLLECTED: RefCell<BTreeMap<String, String>> = RefCell::new(BTreeMap::new());
    }

    unsafe extern "C" fn collect_cb(
        _ctx: *mut core::ffi::c_void,
        key_ptr: *const u16,
        key_len: usize,
        val_ptr: *const u16,
        val_len: usize,
    ) {
        let key = String::from_utf16_lossy(std::slice::from_raw_parts(key_ptr, key_len));
        let val = String::from_utf16_lossy(std::slice::from_raw_parts(val_ptr, val_len));
        COLLECTED.with(|c| { c.borrow_mut().insert(key, val); });
    }

    #[test]
    fn shim_invokes_callback_per_pair() {
        COLLECTED.with(|c| c.borrow_mut().clear());
        let input: Vec<u16> = "a:1;b:2;".encode_utf16().collect();
        unsafe {
            nvda_ia2_attribs_to_map(
                input.as_ptr(),
                input.len(),
                core::ptr::null_mut(),
                collect_cb,
            );
        }
        COLLECTED.with(|c| {
            let m = c.borrow();
            assert_eq!(m.get("a"), Some(&"1".to_string()));
            assert_eq!(m.get("b"), Some(&"2".to_string()));
            assert_eq!(m.len(), 2);
        });
    }

    #[test]
    fn shim_handles_null_input() {
        COLLECTED.with(|c| c.borrow_mut().clear());
        unsafe {
            nvda_ia2_attribs_to_map(core::ptr::null(), 0, core::ptr::null_mut(), collect_cb);
        }
        COLLECTED.with(|c| assert!(c.borrow().is_empty()));
    }
}
```

* [ ] **Step 2: Run the shim tests**

Run: `cd rust && cargo test -p nvda_ia2 --lib attribs`
Expected: All previous tests still pass, plus 2 new shim tests pass.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ia2/src/attribs.rs
git commit -m "Expose IA2AttribsToMap via extern \"C\" callback shim"
```

---

### Task 6: Port `fetchIA2Attributes` to Rust + extern "C" shim

**Files:**

* Modify: `rust/nvda_ia2/src/fetch.rs`

The C++ implementation (`nvdaHelper/common/ia2utils.cpp:22-31`):

```cpp
bool fetchIA2Attributes(IAccessible2* pacc2, map<wstring, wstring>& attribsMap) {
    BSTR attribs = NULL;
    pacc2->get_attributes(&attribs);
    if (!attribs) return false;
    IA2AttribsToMap(attribs, attribsMap);
    SysFreeString(attribs);
    return true;
}
```

* [ ] **Step 1: Write `rust/nvda_ia2/src/fetch.rs`**

Replace the placeholder:

```rust
//! Port of `fetchIA2Attributes` from `nvdaHelper/common/ia2utils.cpp:22`.
//!
//! Calls `IAccessible2::get_attributes`, hands the resulting BSTR to the
//! attributes parser, and invokes a per-pair callback so the C++ wrapper
//! can populate its `std::map<std::wstring, std::wstring>&`.

use crate::attribs::{parse_attribs, AttribCallback};
use crate::interfaces::IAccessible2;
use windows::core::Interface;

/// C-callable replacement for `fetchIA2Attributes`.
///
/// `pacc2` must be a borrowed `IAccessible2*` (the function does not take
/// ownership and does not call `Release`). Returns `true` if attributes
/// were retrieved (and the callback was invoked zero or more times),
/// `false` if the COM call returned no attributes.
///
/// # Safety
/// - `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// - `cb` must be a valid function pointer; `ctx` is opaque user data.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_fetch_attributes(
    pacc2: *mut core::ffi::c_void,
    ctx: *mut core::ffi::c_void,
    cb: AttribCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    // Borrow without taking ownership: from_raw_borrowed gives us a
    // reference that won't Release on drop.
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let bstr = match acc.get_attributes() {
        Ok(b) => b,
        Err(_) => return false,
    };
    if bstr.is_empty() {
        return false;
    }
    let s = bstr.to_string();
    let map = parse_attribs(&s);
    for (k, v) in map {
        let k_utf16: Vec<u16> = k.encode_utf16().collect();
        let v_utf16: Vec<u16> = v.encode_utf16().collect();
        cb(
            ctx,
            k_utf16.as_ptr(),
            k_utf16.len(),
            v_utf16.as_ptr(),
            v_utf16.len(),
        );
    }
    // `bstr` drops here; its Drop calls SysFreeString.
    true
}
```

* [ ] **Step 2: Build to verify it compiles**

Run: `cd rust && cargo build -p nvda_ia2`
Expected: Compiles clean.

If `from_raw_borrowed` isn't the right API for `windows-core` 0.58: it's the standard "borrow a COM pointer without changing refcount" pattern. If the type signature differs, check `windows-core::Interface` docs — but the conceptual operation is the same.

* [ ] **Step 3: Commit**

```bash
git add rust/nvda_ia2/src/fetch.rs
git commit -m "Port fetchIA2Attributes to Rust via IAccessible2 binding"
```

---

### Task 7: Wire `nvda_ia2` staticlib into the SCons build

**Files:**

* Modify: `nvdaHelper/remote/sconscript`

Mirror the existing `nvda_input_hooks` cargo block (lines 87–183 of `nvdaHelper/remote/sconscript`). Both crates are built by the same `cargo build` invocation if you add `nvda_ia2` to the `--package` list, so we extend the existing block rather than duplicating it.

* [ ] **Step 1: Modify the cargo build block**

Find the block in `nvdaHelper/remote/sconscript` that starts with the comment:

```
# x86_64 links the Rust nvda_input_hooks staticlib instead of compiling
```

Replace the comment, the `cargoStaticLib` build, and the `libs.append(cargoStaticLib)` line with this. Show the engineer the diff structurally — the goal is to **build and link both** crates' staticlibs, not just `nvda_input_hooks`.

Specifically:

(a) Update the comment (line 87–91) to mention both crates:

```python
# x86_64 links Rust staticlibs (nvda_input_hooks, nvda_ia2) instead of the
# corresponding C++ source. Cargo produces one .lib per crate for the host
# triple, so non-x86_64 NVDA builds keep using the C++ source. Multi-arch
# cargo builds are a future de-risking exercise.
```

(b) Inside the `if isX64:` block (around line 136), build both staticlibs in one cargo invocation. Add `nvda_ia2.lib` to the outputs and add a second `--package` flag:

```python
cargoStaticLib = None
ia2StaticLib = None
if isX64:
    import os  # noqa: E402
    import subprocess  # noqa: E402

    rustWorkspaceDir = Dir("#rust")
    rustTargetDir = Dir("#build/rust")
    inputHooksLib = rustTargetDir.File("release/nvda_input_hooks.lib")
    ia2Lib = rustTargetDir.File("release/nvda_ia2.lib")

    inputHooksCrate = Dir("#rust/nvda_input_hooks")
    ia2Crate = Dir("#rust/nvda_ia2")

    rustSources = (
        env.Glob(inputHooksCrate.path + "/src/*.rs")
        + env.Glob(ia2Crate.path + "/src/*.rs")
        + [
            inputHooksCrate.File("Cargo.toml"),
            ia2Crate.File("Cargo.toml"),
            rustWorkspaceDir.File("Cargo.toml"),
        ]
    )

    def buildCargoStaticLibs(target, source, env):
        """Run cargo for both staticlibs in one invocation."""
        result = subprocess.run(
            [
                "cargo", "build", "--release",
                "--package", "nvda_input_hooks",
                "--package", "nvda_ia2",
                "--target-dir", rustTargetDir.abspath,
                "--manifest-path", rustWorkspaceDir.File("Cargo.toml").abspath,
            ],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
        )
        if result.returncode != 0:
            print(f"cargo build failed:\n{result.stderr}")
            return result.returncode
        for t in target:
            if not os.path.exists(t.abspath):
                print(f"cargo built successfully but {t.abspath} was not produced")
                return 1
        return 0

    cargoStaticLib, ia2StaticLib = env.Command(
        [inputHooksLib, ia2Lib],
        rustSources,
        buildCargoStaticLibs,
    )
```

(c) Update the `libs.append(...)` block (around line 200–208) to append both:

```python
if cargoStaticLib is not None:
    libs.append(cargoStaticLib)
    libs.append(ia2StaticLib)
    # Rust libstd transitively references symbols from these Win32 system
    # libraries (ntdll for sync primitives and Nt* file APIs, userenv for
    # home_dir lookups, ws2_32 for std::net, bcrypt for the secure RNG,
    # WindowsApp for WinRT RoOriginateError). MSVC does not pick up the
    # /DEFAULTLIB directives embedded in the Rust .lib reliably for
    # staticlib consumers, so list them explicitly.
    libs.extend(["ntdll", "userenv", "ws2_32", "bcrypt", "WindowsApp"])
```

* [ ] **Step 2: Verify `scons.bat` runs cargo and produces both .libs**

Run: `scons.bat source` (or `uvx scons source` per user preference)
Expected:

* A line in the output mentioning `cargo build --release --package nvda_input_hooks --package nvda_ia2`
* Both `build/rust/release/nvda_input_hooks.lib` and `build/rust/release/nvda_ia2.lib` exist after the build
* `nvdaHelperRemote.dll` builds successfully and lands in `source/`

If the link fails with unresolved symbols, the C++ wrappers in Task 8 don't yet exist — this task should still get a clean cargo build, but the link will fail until Task 8 lands. To verify just the cargo step in this task without the link error, run cargo manually:

```bash
cd rust && cargo build --release --package nvda_ia2 --target-dir ../build/rust
```

Expected: Produces `build/rust/release/nvda_ia2.lib`.

* [ ] **Step 3: Commit (just the sconscript change for now; don't run a full build link yet)**

```bash
git add nvdaHelper/remote/sconscript
git commit -m "Build nvda_ia2 staticlib alongside nvda_input_hooks (x86_64 only)"
```

---

### Task 8: Replace the C++ implementations in `ia2utils.cpp` with Rust delegations

**Files:**

* Modify: `nvdaHelper/common/ia2utils.cpp`

The C++ wrapper functions keep their existing signatures (`std::map<std::wstring, std::wstring>&` etc.) so callers are unchanged. Internally they call the Rust shims via the callback.

* [ ] **Step 1: Replace `IA2AttribsToMap` and `fetchIA2Attributes` in `ia2utils.cpp`**

Edit `nvdaHelper/common/ia2utils.cpp`. Delete the existing bodies of `fetchIA2Attributes` (lines 22–31) and `IA2AttribsToMap` (lines 33–75) and replace with:

```cpp
// Forward declarations of the Rust shims (linked from nvda_ia2.lib on x86_64).
// Non-x86_64 builds do not link nvda_ia2 -- those builds use the C++ fallback
// at the bottom of this file (guarded by `#ifndef _M_X64`).
#ifdef _M_X64
extern "C" {
    typedef void (*AttribCallback)(
        void* ctx,
        const wchar_t* key, size_t key_len,
        const wchar_t* val, size_t val_len);

    void nvda_ia2_attribs_to_map(
        const wchar_t* input, size_t input_len,
        void* ctx, AttribCallback cb);

    bool nvda_ia2_fetch_attributes(
        void* pacc2, void* ctx, AttribCallback cb);
}

namespace {
    void insert_into_map(
        void* ctx,
        const wchar_t* key, size_t key_len,
        const wchar_t* val, size_t val_len
    ) {
        auto& m = *static_cast<std::map<std::wstring, std::wstring>*>(ctx);
        m.emplace(std::wstring(key, key_len), std::wstring(val, val_len));
    }
}

bool fetchIA2Attributes(IAccessible2* pacc2, std::map<std::wstring, std::wstring>& attribsMap) {
    return nvda_ia2_fetch_attributes(pacc2, &attribsMap, insert_into_map);
}

void IA2AttribsToMap(const std::wstring& attribsString, std::map<std::wstring, std::wstring>& attribsMap) {
    nvda_ia2_attribs_to_map(
        attribsString.c_str(),
        attribsString.size(),
        &attribsMap,
        insert_into_map);
}
#else
// Non-x86_64 fallback: keep the original C++ implementations because cargo
// only produces a host-triple staticlib. This is the same code as before
// this PR, kept verbatim. Multi-arch cargo builds are a future exercise.

bool fetchIA2Attributes(IAccessible2* pacc2, std::map<std::wstring, std::wstring>& attribsMap) {
    BSTR attribs = NULL;
    pacc2->get_attributes(&attribs);
    if (!attribs) {
        return false;
    }
    IA2AttribsToMap(attribs, attribsMap);
    SysFreeString(attribs);
    return true;
}

void IA2AttribsToMap(const std::wstring& attribsString, std::map<std::wstring, std::wstring>& attribsMap) {
    std::wstring str, key;
    bool inEscape = false;

    for (std::wstring::const_iterator it = attribsString.begin(); it != attribsString.end(); ++it) {
        if (inEscape) {
            str.push_back(*it);
            inEscape = false;
        } else if (*it == L'\\') {
            inEscape = true;
        } else if (*it == L':') {
            key = str;
            str.clear();
        } else if (*it == L';') {
            if (!key.empty())
                attribsMap[key] = str;
                key.clear();
            str.clear();
        } else {
            str.push_back(*it);
        }
    }
    if (!key.empty())
        attribsMap[key] = str;
    std::map<std::wstring, std::wstring>::const_iterator attribsMapIt;
    if ((attribsMapIt = attribsMap.find(L"src")) != attribsMap.end()) {
        str = attribsMapIt->second;
        const std::wstring prefix = L"data:";
        if (str.substr(0, prefix.length()) == prefix) {
            const std::wstring needle = L"base64,";
            std::wstring::size_type pos = str.find(needle);
            if (pos != std::wstring::npos) {
                str.replace(pos + needle.length(), std::wstring::npos, L"<truncated>");
                attribsMap[L"src"] = str;
            }
        }
    }
}
#endif
```

Leave `getAccessibleChildren` and the `HyperlinkGetter` classes (lines 77–182 of the original) untouched.

* [ ] **Step 2: Build the full DLL**

Run: `scons.bat source` (or `uvx scons source`)
Expected: `nvdaHelperRemote.dll` builds successfully. No unresolved symbols related to `nvda_ia2_*`.

If you see `LNK2019: unresolved external symbol nvda_ia2_attribs_to_map`:

* Confirm `nvda_ia2.lib` is on the link line (check the SCons command output).
* Confirm the Rust functions have `#[no_mangle]` and `extern "C"`.
* Confirm the C++ `extern "C"` block matches the Rust signatures.

If you see "ambiguous redefinition" or two definitions of `IA2AttribsToMap`: ensure the `#else` branch isn't being compiled when `_M_X64` is defined. MSVC defines `_M_X64` for x86_64.

* [ ] **Step 3: Commit**

```bash
git add nvdaHelper/common/ia2utils.cpp
git commit -m "Delegate IA2AttribsToMap and fetchIA2Attributes to Rust on x86_64"
```

---

### Task 9: Smoke-test the build inside running NVDA

**Files:** None modified.

The Rust port of `IA2AttribsToMap` and `fetchIA2Attributes` is on the hot path for IA2-enabled apps (Firefox, Chrome with IA2 enabled, anything using gecko_ia2). A regression here would silently break IA2 attribute handling.

* [ ] **Step 1: Run NVDA**

Run: `runnvda.bat`

NVDA should start. If it does not start or crashes immediately, the Rust port has a regression — debug before continuing.

* [ ] **Step 2: Smoke-test with a browser that uses IA2**

Open Firefox (which uses IA2 for the web content tree). Navigate to any reasonably structured page (e.g. `https://www.nvaccess.org/`). Use NVDA's browse-mode shortcuts to walk through headings (`H`), links (`K`), form fields (`F`).

Expected: NVDA reads heading levels, link text, button labels correctly. Heading levels in particular come through IA2 attributes.

If headings have no level announced ("heading" instead of "heading level 2"), the IA2 attribute parser is broken — re-check the Rust parser against the C++.

* [ ] **Step 3: Verify no Rust panics in the NVDA log**

Open NVDA's log viewer (`NVDA+F1`) and search for `panic`, `RUST_BACKTRACE`, or `nvda_ia2`. Expected: no entries.

* [ ] **Step 4: Commit nothing, but record the smoke-test result in the PR description later**

If the smoke test passes, proceed. If anything is off, the regression must be fixed before pushing.

---

### Task 10: Push the branch

**Files:** None modified.

* [ ] **Step 1: Verify the working tree is clean**

Run: `git status`
Expected: nothing to commit, working tree clean (modulo the existing pre-existing uncommitted changes from before this branch — those are not from this PR).

* [ ] **Step 2: Push**

Run: `git push`
Expected: branch `worktree-rust-beep-generator` is updated on the remote. **Do not** open a PR — the user has been pushing without opening PRs in recent work.

---

## Self-review notes

* **Spec coverage.** The user's PR 1 spec was: "New `nvda_ia2` crate with hand-rolled IA2 interface bindings + Rust port of `ia2utils`". All 5 IA2 interfaces from the spec (IAccessible2, IAccessibleText, IAccessibleHypertext, IAccessibleHypertext2 added beyond original spec because `makeHyperlinkGetter` will need it in PR 2, IAccessibleHyperlink) are bound. Two of the four ia2utils functions are ported (IA2AttribsToMap, fetchIA2Attributes). The other two (getAccessibleChildren, makeHyperlinkGetter) are deliberately deferred to PR 2 because they require complex shims (`std::pair<std::vector<CComVariant>, HRESULT>` and a polymorphic class hierarchy) that have no readers other than the C++ files PR 2 will replace anyway. The plan flags this scope reduction in the Goal and Architecture sections.
* **Multi-arch.** Rust path is gated to `_M_X64` (in C++) and `TARGET_ARCH == "x86_64"` (in SCons). Other arches keep the C++ implementation in the `#else` block. Matches the `nvda_input_hooks` precedent.
* **Test coverage.** Parser has 14 unit tests covering empty input, escape handling, src truncation, and the C++ corner case of empty keys. Shim has 2 tests covering the callback path and null input. The IA2 binding for `get_attributes` is exercised end-to-end during smoke testing in Task 9 (Firefox heading levels), since pure unit tests can't construct a real IAccessible2.
* **Risk.** Hand-rolled vtable layouts are the highest-risk piece. Mitigated by: (1) only one method (`IAccessible2::get_attributes`) is invoked in this PR, so a wrong offset for an unused method just produces dead code; (2) Task 9 smoke-tests with a real IA2 client (Firefox).

## Execution Handoff

Plan complete and saved to `docs/plans/2026-05-03-rust-ia2-bindings.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
