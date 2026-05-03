# Rust port of `getTextFromIAccessible` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `getTextFromIAccessible` from `nvdaHelper/remote/textFromIAccessible.cpp` to Rust on x86_64; keep the public C++ signature unchanged.

**Architecture:** Mirror PR 1 (the `nvda_ia2` crate created by `docs/plans/2026-05-03-rust-ia2-bindings.md`): Rust port lives in a new module of the existing `nvda_ia2` staticlib, exposed via an `extern "C"` callback-based shim. C++ `getTextFromIAccessible` becomes a thin wrapper on x86_64; non-x86_64 keeps the verbatim C++ body under `#ifdef _M_X64`.

**Tech Stack:** Rust 2021, `windows-core 0.58`, `windows 0.58` (features unchanged from PR 1: `Win32_Foundation`, `Win32_System_Com`, `Win32_UI_Accessibility`).

**Reference design:** `docs/plans/2026-05-03-rust-text-from-iaccessible-design.md`.

---

## Background you will need

* **PR 1 already landed.** The `nvda_ia2` crate exists at `rust/nvda_ia2/` with hand-rolled vtables for `IAccessible2`, `IAccessibleText`, `IAccessibleHypertext`, `IAccessibleHypertext2`, `IAccessibleHyperlink`. Vtable slot declarations in `interfaces.rs` already include the methods this plan needs. Only Rust *method wrappers* (the `impl` blocks calling through `Interface::vtable(self)`) are missing for the IAccessibleText / IAccessibleHypertext methods.
* **Pattern to mirror.** The existing `IAccessible2::get_attributes` wrapper (in `rust/nvda_ia2/src/interfaces.rs:106`) and the existing `nvda_ia2_fetch_attributes` shim (in `rust/nvda_ia2/src/fetch.rs`) are the templates. New wrappers should match their `# Safety` doc style and BSTR drop-on-error pattern.
* **`IA2_TEXT_OFFSET_LENGTH = -1`** per `include/ia2/api/IA2CommonTypes.idl:160`. Pass as `i32` to `get_text`.
* **`OBJ_REPLACEMENT_CHAR = 0xFFFC`** (the C++ code uses this constant; declare a Rust `const` in the `text` module).
* **C ABI exception safety.** All `extern "C"` callbacks take a `void* ctx` opaque to Rust. The C++ side must catch `std::bad_alloc` (or accept process termination on OOM) before returning to Rust — same constraint as PR 1's `AttribCallback`. Document it in the shim's `# Safety` block.
* **`BSTR` ownership.** `windows::core::BSTR` calls `SysFreeString` on `Drop`. For `_Vtbl` slots that take `*mut ManuallyDrop<BSTR>`, the wrapper takes ownership of the written BSTR by `ManuallyDrop::into_inner` — matching `IAccessible2::get_attributes`.
* **QI mapping.** `IUnknown::cast::<T>()` returns `Result<T>`. We want `Option<T>` semantics (silent fallback on failure, matching `CComQIPtr`'s null behavior). Use `.cast::<T>().ok()`.

## File structure

**Modify:**

| File | Change |
| --- | --- |
| `rust/nvda_ia2/src/interfaces.rs` | Add 4 Rust method wrappers (Tasks 1-2). Vtable slot declarations are unchanged. |
| `rust/nvda_ia2/src/lib.rs` | Add `pub mod text;`. |
| `nvdaHelper/remote/textFromIAccessible.cpp` | Replace function body with `#ifdef _M_X64` Rust shim delegation; preserve verbatim C++ in `#else` branch (Task 5). |

**Create:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/src/text.rs` | `get_text_from_iaccessible` Rust port, `is_empty_text` pure helper with unit tests, `extern "C"` shim. |

---

## Task 1: Add `IAccessibleText` Rust method wrappers

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs` (add `impl IAccessibleText { ... }` block after the existing struct, around line 190)

* [ ] **Step 1: Add the `impl IAccessibleText` block**

Insert this immediately after the closing `}` of the existing `pub struct IAccessibleText_Vtbl { ... }` definition in `rust/nvda_ia2/src/interfaces.rs`:

```rust
impl IAccessibleText {
    /// Returns `[start_offset, end_offset)` of the text. Pass `0` /
    /// `IA2_TEXT_OFFSET_LENGTH` (-1) to retrieve the whole string.
    /// See `include/ia2/api/AccessibleText.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleText` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_text(&self, start_offset: i32, end_offset: i32) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_text)(
            Interface::as_raw(self),
            start_offset,
            end_offset,
            &mut out as *mut _,
        );
        if hr.is_err() {
            // Drop any BSTR a misbehaving server may have written before
            // returning failure.
            let _ = core::mem::ManuallyDrop::into_inner(out);
            return Err(hr.into());
        }
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }

    /// Returns the most recently inserted text segment for this object.
    /// Only valid during an in-process winEvent callback (IA2_EVENT_TEXT_*).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleText` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`]. The returned `IA2TextSegment` owns
    /// its `text` BSTR (freed on drop).
    pub unsafe fn get_newText(&self) -> windows::core::Result<IA2TextSegment> {
        let mut out = IA2TextSegment::default();
        let hr = (Interface::vtable(self).get_newText)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            // Drop any BSTR a misbehaving server may have written before
            // returning failure. `IA2TextSegment::Drop` is BSTR's Drop on the
            // `text` field, which calls SysFreeString.
            return Err(hr.into());
        }
        Ok(out)
    }
}
```

* [ ] **Step 2: Verify the crate still builds and clippy is clean**

Run from `rust/`:

```sh
cargo build --package nvda_ia2
cargo clippy --package nvda_ia2 --all-targets -- -D warnings
```

Expected: clean build, no warnings.

* [ ] **Step 3: Commit**

```sh
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add Rust wrappers for IAccessibleText::get_text and get_newText"
```

---

## Task 2: Add `IAccessibleHypertext` Rust method wrappers

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs` (add `impl IAccessibleHypertext { ... }` block after the existing `IAccessibleHypertext_Vtbl` struct, around line 218)

* [ ] **Step 1: Add the `impl IAccessibleHypertext` block**

Insert this immediately after the closing `}` of the existing `pub struct IAccessibleHypertext_Vtbl { ... }` definition in `rust/nvda_ia2/src/interfaces.rs`:

```rust
impl IAccessibleHypertext {
    /// Retrieves the hyperlink at `index`. The COM contract returns
    /// `E_INVALIDARG` when `index >= n_hyperlinks`. The caller is expected
    /// to bound-check via `get_hyperlinkIndex` first (the pattern used by
    /// `getTextFromIAccessible`).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext` implementation for the duration of
    /// this call.
    pub unsafe fn get_hyperlink(&self, index: i32) -> windows::core::Result<IAccessibleHyperlink> {
        let mut out: Option<IAccessibleHyperlink> = None;
        let hr = (Interface::vtable(self).get_hyperlink)(
            Interface::as_raw(self),
            index,
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        out.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))
    }

    /// Returns the hyperlink index for the embedded-object character at
    /// `char_index`, or an HRESULT error if there is no hyperlink at that
    /// offset.
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext` implementation for the duration of
    /// this call.
    pub unsafe fn get_hyperlinkIndex(&self, char_index: i32) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_hyperlinkIndex)(
            Interface::as_raw(self),
            char_index,
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }
}
```

* [ ] **Step 2: Verify the crate still builds and clippy is clean**

Run from `rust/`:

```sh
cargo build --package nvda_ia2
cargo clippy --package nvda_ia2 --all-targets -- -D warnings
```

Expected: clean build, no warnings.

* [ ] **Step 3: Commit**

```sh
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add Rust wrappers for IAccessibleHypertext::get_hyperlink and get_hyperlinkIndex"
```

---

## Task 3: Add `is_empty_text` pure helper with tests (TDD)

**Files:**

* Create: `rust/nvda_ia2/src/text.rs`
* Modify: `rust/nvda_ia2/src/lib.rs` (add `pub mod text;`)

This task creates the `text` module with only the pure helper (and unit tests). The `extern "C"` shim and the `get_text_from_iaccessible` body are added in Task 4.

* [ ] **Step 1: Create the module file with the failing tests**

Create `rust/nvda_ia2/src/text.rs` with:

```rust
//! Port of `getTextFromIAccessible` from
//! `nvdaHelper/remote/textFromIAccessible.cpp`.
//!
//! For now this module exposes only the `is_empty_text` pure helper.
//! The full `get_text_from_iaccessible` port and its `extern "C"` shim
//! are added in a follow-up commit.

pub const OBJ_REPLACEMENT_CHAR: u16 = 0xFFFC;

/// Mirrors the C++ `isEmpty` helper in
/// `nvdaHelper/remote/textFromIAccessible.cpp:27`. A text run is "empty"
/// for our purposes if every character is either whitespace or the
/// embedded-object replacement character.
pub fn is_empty_text(chars: &[u16]) -> bool {
    chars.iter().all(|&c| c == OBJ_REPLACEMENT_CHAR || is_whitespace_w(c))
}

/// Mirrors the C runtime `iswspace` for the BMP characters NVDA actually
/// sees through BSTRs. The C++ code calls `iswspace` directly; we
/// implement the standard whitespace set ourselves to keep this a pure
/// Rust function (testable without the CRT).
fn is_whitespace_w(c: u16) -> bool {
    matches!(
        c,
        0x0009 // tab
        | 0x000A // line feed
        | 0x000B // vertical tab
        | 0x000C // form feed
        | 0x000D // carriage return
        | 0x0020 // space
        | 0x00A0 // no-break space (iswspace returns true for this in many locales,
                 // and NVDA encounters it from web content)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slice_is_empty() {
        assert!(is_empty_text(&[]));
    }

    #[test]
    fn all_spaces_is_empty() {
        let chars: Vec<u16> = "    ".encode_utf16().collect();
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn all_object_replacement_is_empty() {
        assert!(is_empty_text(&[OBJ_REPLACEMENT_CHAR; 5]));
    }

    #[test]
    fn mixed_spaces_and_object_replacement_is_empty() {
        let mut chars: Vec<u16> = " ".encode_utf16().collect();
        chars.push(OBJ_REPLACEMENT_CHAR);
        chars.extend("\t\n".encode_utf16());
        chars.push(OBJ_REPLACEMENT_CHAR);
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn single_letter_is_not_empty() {
        let chars: Vec<u16> = "a".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn whitespace_around_letter_is_not_empty() {
        let chars: Vec<u16> = "  a  ".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn nbsp_alone_is_empty() {
        assert!(is_empty_text(&[0x00A0]));
    }
}
```

* [ ] **Step 2: Add the module to the crate root**

Modify `rust/nvda_ia2/src/lib.rs`. Find the existing module declarations:

```rust
pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod types;
```

Replace with:

```rust
pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod text;
pub mod types;
```

* [ ] **Step 3: Run the tests, confirm they pass**

Run from `rust/`:

```sh
cargo test --package nvda_ia2 --lib text::
```

Expected: 7 tests pass (`empty_slice_is_empty`, `all_spaces_is_empty`, `all_object_replacement_is_empty`, `mixed_spaces_and_object_replacement_is_empty`, `single_letter_is_not_empty`, `whitespace_around_letter_is_not_empty`, `nbsp_alone_is_empty`).

* [ ] **Step 4: Verify clippy stays clean**

Run from `rust/`:

```sh
cargo clippy --package nvda_ia2 --all-targets -- -D warnings
```

Expected: no warnings.

* [ ] **Step 5: Commit**

```sh
git add rust/nvda_ia2/src/text.rs rust/nvda_ia2/src/lib.rs
git commit -m "Add is_empty_text pure helper for text-from-IAccessible port"
```

---

## Task 4: Implement `get_text_from_iaccessible` and the `extern "C"` shim

**Files:**

* Modify: `rust/nvda_ia2/src/text.rs` (append the full port + shim)

* [ ] **Step 1: Add the port + shim to `text.rs`**

Append the following to `rust/nvda_ia2/src/text.rs` (after the existing `is_whitespace_w` function, before the `#[cfg(test)]` block):

```rust
use crate::interfaces::{IAccessible2, IAccessibleHypertext, IAccessibleText};
use std::collections::BTreeMap;
use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::UI::Accessibility::{AccessibleChildren, IAccessible};

/// `IA2_TEXT_OFFSET_LENGTH` per `include/ia2/api/IA2CommonTypes.idl:160`.
const IA2_TEXT_OFFSET_LENGTH: i32 = -1;

/// `VT_DISPATCH` per OAIDL.h. windows-core 0.58 doesn't expose a typed
/// constant we can use against the raw `imp::VARIANT` `vt` field
/// (`u16`-typed in the imp module), so we declare it locally.
const VT_DISPATCH_RAW: u16 = 9;

/// C-callable callback. Invoked once at the end of
/// [`nvda_ia2_get_text_from_iaccessible`] with the accumulated text. Mirrors
/// the C++ `textBuf.append(ptr, len)` pattern.
///
/// # Safety
///
/// The callback must not unwind. The pointer is valid for `len` `u16`
/// elements; the callback must copy the data before returning.
pub type AppendCharsCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    ptr: *const u16,
    len: usize,
);

/// C-callable replacement for `getTextFromIAccessible`.
///
/// `pacc2` is borrowed (no `Release`). On `true`, `cb` was invoked exactly
/// once with the collected text (possibly empty). On `false`, `cb` may have
/// been invoked zero or one times — the C++ caller must accept either.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind. The C++ adapter (`textFromIAccessible.cpp`)
///   must catch any `std::bad_alloc` from `std::wstring::append` (or accept
///   process termination on OOM) before returning to Rust.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_text_from_iaccessible(
    pacc2: *mut core::ffi::c_void,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
    ctx: *mut core::ffi::c_void,
    cb: AppendCharsCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let mut buf: Vec<u16> = Vec::new();
    let got_text = get_text_from_iaccessible(
        &mut buf,
        acc2,
        use_new_text,
        recurse,
        include_top_level_text,
    );
    cb(ctx, buf.as_ptr(), buf.len());
    got_text
}

/// Pure-Rust port of `getTextFromIAccessible`.
fn get_text_from_iaccessible(
    text_buf: &mut Vec<u16>,
    pacc2: &IAccessible2,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
) -> bool {
    let mut got_text = false;
    let pacc_text: Option<IAccessibleText> = pacc2.cast().ok();

    if pacc_text.is_none() && recurse && !use_new_text {
        // No IAccessibleText interface, so try children instead. Mirrors
        // textFromIAccessible.cpp:79-104.
        let pacc: &IAccessible = pacc2; // Deref to the IAccessible base.
        let child_count = match unsafe { pacc.accChildCount() } {
            Ok(n) if n > 0 => n,
            _ => return got_text,
        };
        let mut variants: Vec<VARIANT> = vec![VARIANT::default(); child_count as usize];
        let mut filled: i32 = 0;
        if unsafe { AccessibleChildren(pacc, 0, &mut variants[..], &mut filled) }.is_err() {
            return got_text;
        }
        variants.truncate(filled as usize);
        for v in variants.iter() {
            // VT_DISPATCH child contains an IDispatch we QI to IAccessible2.
            let pdisp = variant_dispatch_ptr(v);
            if pdisp.is_null() {
                continue;
            }
            // Borrow the IDispatch -- the VARIANT owns the reference; we
            // borrow it only long enough to QI/cast. cast() returns a fresh
            // owned IAccessible2 with its own AddRef.
            let disp: &IDispatch = match unsafe { IDispatch::from_raw_borrowed(&pdisp) } {
                Some(d) => d,
                None => continue,
            };
            let pacc2_child: IAccessible2 = match disp.cast() {
                Ok(a) => a,
                Err(_) => continue,
            };
            if child_is_live_off(&pacc2_child) {
                continue;
            }
            got_text |= get_text_from_iaccessible(
                text_buf,
                &pacc2_child,
                false, // use_new_text
                true,  // recurse
                true,  // include_top_level_text
            );
        }
    } else if let Some(pacc_text) = pacc_text.as_ref() {
        // We can use IAccessibleText. Mirrors textFromIAccessible.cpp:105-160.
        // We hold the BSTR alive for the whole loop so as_wide()'s slice
        // stays valid; iterate by index because we need start_offset + idx.
        let (bstr_text, start_offset): (Option<BSTR>, i32) = if use_new_text {
            match unsafe { pacc_text.get_newText() } {
                Ok(mut seg) if !is_bstr_null(&seg.text) => {
                    let start = seg.start;
                    // Move the BSTR out of the segment so we own it independently.
                    let text = core::mem::take(&mut seg.text);
                    (Some(text), start)
                }
                _ => (None, 0),
            }
        } else {
            match unsafe { pacc_text.get_text(0, IA2_TEXT_OFFSET_LENGTH) } {
                Ok(b) if !is_bstr_null(&b) => (Some(b), 0),
                _ => (None, 0),
            }
        };
        if let Some(bstr_text) = bstr_text {
            let chars = bstr_text.as_wide();
            let pacc_hyper: Option<IAccessibleHypertext> = if recurse {
                pacc2.cast().ok()
            } else {
                None
            };
            for (idx, &real_char) in chars.iter().enumerate() {
                let mut char_added = false;
                if real_char == OBJ_REPLACEMENT_CHAR {
                    if let Some(pacc_hyper) = pacc_hyper.as_ref() {
                        let char_index = start_offset + idx as i32;
                        if let Ok(hyperlink_index) =
                            unsafe { pacc_hyper.get_hyperlinkIndex(char_index) }
                        {
                            if let Ok(pacc_hyperlink) =
                                unsafe { pacc_hyper.get_hyperlink(hyperlink_index) }
                            {
                                if let Ok(pacc2_child) =
                                    pacc_hyperlink.cast::<IAccessible2>()
                                {
                                    if !child_is_live_off(&pacc2_child)
                                        && get_text_from_iaccessible(
                                            text_buf, &pacc2_child, false, true, true,
                                        )
                                    {
                                        got_text = true;
                                    }
                                    char_added = true;
                                }
                            }
                        }
                    }
                }
                if !char_added && include_top_level_text {
                    text_buf.push(real_char);
                    if real_char != OBJ_REPLACEMENT_CHAR && !is_whitespace_w(real_char) {
                        got_text = true;
                    }
                }
            }
            text_buf.push(b' ' as u16);
            // bstr_text drops here, freeing the BSTR.
        }
    }

    if !got_text && !use_new_text {
        // Fall back to name and/or description. Mirrors
        // textFromIAccessible.cpp:162-165.
        got_text = append_name_description(text_buf, pacc2);
    }
    got_text
}

/// Mirrors `appendNameDescription` in `textFromIAccessible.cpp:39`.
fn append_name_description(text_buf: &mut Vec<u16>, pacc2: &IAccessible2) -> bool {
    let pacc: &IAccessible = pacc2;
    let varchild = VARIANT::from(0i32); // CHILDID_SELF
    let mut got_text = false;

    if let Ok(name) = unsafe { pacc.get_accName(&varchild) } {
        let chars = name.as_wide();
        if !is_empty_text(chars) {
            text_buf.extend_from_slice(chars);
            text_buf.push(b' ' as u16);
            got_text = true;
        }
    }
    if let Ok(desc) = unsafe { pacc.get_accDescription(&varchild) } {
        let chars = desc.as_wide();
        if !is_empty_text(chars) {
            text_buf.extend_from_slice(chars);
            got_text = true;
        }
    }
    got_text
}

/// Returns true if the `live` IA2 attribute equals `"off"` for `pacc2`.
/// Mirrors the live-region filter at `textFromIAccessible.cpp:90` and
/// `:140`. A failed `get_attributes` (no attributes string, or HRESULT
/// error) does not suppress the child — this matches the C++ behavior,
/// where an absent `live` key falls through to the recursion/append branch.
fn child_is_live_off(pacc2: &IAccessible2) -> bool {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(bstr) = unsafe { pacc2.get_attributes() } {
        if !is_bstr_null(&bstr) {
            map = crate::attribs::parse_attribs(&bstr.to_string());
        }
    }
    matches!(map.get("live"), Some(v) if v == "off")
}

/// Extract the `pdispVal` raw pointer from a `VARIANT` if its `vt` is
/// `VT_DISPATCH`. Returns `null` for any other VARENUM (including
/// `VT_I4`, which `AccessibleChildren` may also produce). The pointer
/// is non-owning — the VARIANT retains the AddRef.
///
/// SAFETY: `windows_core::imp::VARIANT` is the same `VARIANT` C structure
/// from `OAIDL.h`. Reading `vt` is always safe (it's a discriminant); we
/// only read `pdispVal` after confirming `vt == VT_DISPATCH`, which is the
/// VARENUM contract for that union member being active.
fn variant_dispatch_ptr(v: &VARIANT) -> *mut core::ffi::c_void {
    let raw = v.as_raw();
    let inner = unsafe { &raw.Anonymous.Anonymous };
    if inner.vt != VT_DISPATCH_RAW {
        return core::ptr::null_mut();
    }
    unsafe { inner.Anonymous.pdispVal }
}

/// `BSTR::is_empty()` returns true for both NULL and zero-length BSTRs.
/// We need to distinguish: a zero-length BSTR is treated as "got the call
/// back, but no text" (still trigger the trailing-space append at the end
/// of the text branch) while a NULL BSTR is "the call returned nothing
/// usable" (skip the branch entirely). Mirrors the trick used in
/// `fetch.rs`. SAFETY: `windows::core::BSTR` is `#[repr(transparent)]`
/// over a single `*const u16` field; verified in
/// `windows-strings-0.1.0/src/bstr.rs:6`.
fn is_bstr_null(bstr: &BSTR) -> bool {
    let raw_ptr: *const u16 = unsafe { *(bstr as *const _ as *const *const u16) };
    raw_ptr.is_null()
}
```

* [ ] **Step 2: Verify the crate builds and clippy is clean**

Run from `rust/`:

```sh
cargo build --package nvda_ia2
cargo clippy --package nvda_ia2 --all-targets -- -D warnings
```

Expected: clean build, no warnings. Note: any clippy diagnostics about `if let` chains, matches, or needless borrows should be respected — fix them in place rather than allow-ing.

* [ ] **Step 3: Re-run the existing tests to confirm no regressions**

Run from `rust/`:

```sh
cargo test --package nvda_ia2
```

Expected: all existing tests still pass (the 20 from PR 1 plus the 7 added in Task 3 = 27 total).

* [ ] **Step 4: Commit**

```sh
git add rust/nvda_ia2/src/text.rs
git commit -m "Port getTextFromIAccessible to Rust with extern C shim"
```

---

## Task 5: Wire the C++ delegation in `textFromIAccessible.cpp`

**Files:**

* Modify: `nvdaHelper/remote/textFromIAccessible.cpp`

The PR 1 SCons changes already build `nvda_ia2.lib` and link it into `nvdaHelperRemote.dll` on x86_64. No build-system changes are needed for this task.

* [ ] **Step 1: Replace the function body**

Replace the entire contents of `nvdaHelper/remote/textFromIAccessible.cpp` with the following. The existing helpers (`isEmpty`, `appendNameDescription`) are kept inside the `#else` branch since they are no longer used by the `#ifdef _M_X64` path:

```cpp
/*
This file is a part of the NVDA project.
Copyright 2006-2021 NV Access Limited
	This program is free software: you can redistribute it and/or modify
	it under the terms of the GNU General Public License version 2.0, as published by
	the Free Software Foundation.
	This program is distributed in the hope that it will be useful,
	but WITHOUT ANY WARRANTY; without even the implied warranty of
	MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include "textFromIAccessible.h"
#include <string>
#include <vector>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <atlcomcli.h>
#include <ia2.h>
#include <common/ia2utils.h>

using namespace std;
auto constexpr OBJ_REPLACEMENT_CHAR = L'\xfffc';

#ifdef _M_X64
extern "C" {
	typedef void (*AppendCharsCallback)(
		void* ctx,
		const wchar_t* ptr,
		size_t len);

	bool nvda_ia2_get_text_from_iaccessible(
		void* pacc2,
		bool use_new_text,
		bool recurse,
		bool include_top_level_text,
		void* ctx,
		AppendCharsCallback cb);
}

namespace {
	void append_chars(void* ctx, const wchar_t* ptr, size_t len) {
		try {
			static_cast<std::wstring*>(ctx)->append(ptr, len);
		} catch (const std::bad_alloc&) {
			// Swallow OOM rather than letting an exception cross the C ABI
			// frame back into Rust. The text buffer will be partially
			// populated; matches the C++ implementation's pre-PR-2 behavior
			// (which would also fail on OOM).
		}
	}
}

bool getTextFromIAccessible(
	wstring& textBuf,
	IAccessible2* pacc2,
	bool useNewText,
	bool recurse,
	bool includeTopLevelText
) {
	return nvda_ia2_get_text_from_iaccessible(
		pacc2, useNewText, recurse, includeTopLevelText,
		&textBuf, append_chars);
}

#else
// Non-x86_64 fallback: keep the original C++ implementation because cargo
// only produces a host-triple staticlib. This is the same code as before
// this PR, kept verbatim. Multi-arch cargo builds are a future exercise.

bool isEmpty(CComBSTR& val) {
	if (!val) {
		return true;
	}
	for (int i = 0; val[i] != L'\0'; ++i) {
		if (val[i] != OBJ_REPLACEMENT_CHAR && !iswspace(val[i])) {
			return false;
		}
	}
	return true;
}

bool appendNameDescription(CComPtr<IAccessible> pacc, wstring& textBuf) {
	bool gotText = false;
	CComVariant varChild;
	varChild.vt = VT_I4;
	varChild.lVal = 0;

	CComBSTR val;
	pacc->get_accName(varChild, &val);
	bool valEmpty = isEmpty(val);
	if (!valEmpty) {
		gotText = true;
		textBuf.append(val);
		textBuf.append(L" ");
	}

	val = nullptr;
	pacc->get_accDescription(varChild, &val);
	valEmpty = isEmpty(val);
	if (!valEmpty) {
		gotText = true;
		textBuf.append(val);
	}
	return gotText;
}

bool getTextFromIAccessible(
	wstring& textBuf,
	IAccessible2* pacc2,
	bool useNewText,
	bool recurse,
	bool includeTopLevelText
) {
	if (!pacc2) {
		return false;
	}

	bool gotText = false;
	CComQIPtr<IAccessibleText> paccText(pacc2);

	if (!paccText && recurse && !useNewText) {
		long childCount = 0;
		if (!useNewText && pacc2->get_accChildCount(&childCount) == S_OK && childCount > 0) {
			auto[varChildren, accChildRes] = getAccessibleChildren(pacc2, 0, childCount);
			for(auto& child : varChildren){
				if (child.vt == VT_DISPATCH && child.pdispVal) {
					CComQIPtr<IAccessible2> pacc2Child(child.pdispVal);
					if (pacc2Child) {
						map<wstring, wstring> childAttribsMap;
						fetchIA2Attributes(pacc2Child, childAttribsMap);
						auto liveItr = childAttribsMap.find(L"live");
						if (liveItr == childAttribsMap.end() || liveItr->second.compare(L"off") != 0) {
							gotText |= getTextFromIAccessible(
								textBuf,
								pacc2Child,
								false,
								true,
								true
							);
						}
					}
				}
			}
		}
	}
	else if (paccText) {
		CComBSTR bstrText;
		long startOffset = 0;
		if (useNewText) {
			IA2TextSegment newSeg {};
			if (S_OK == paccText->get_newText(&newSeg) && newSeg.text) {
				bstrText = newSeg.text;
				startOffset = newSeg.start;
			}
		}
		else {
			paccText->get_text(0, IA2_TEXT_OFFSET_LENGTH, &bstrText);
		}
		if (bstrText) {
			long textLength = SysStringLen(bstrText);
			CComQIPtr<IAccessibleHypertext> paccHypertext;
			if (recurse) {
				paccHypertext = pacc2;
			}
			for (long index = 0; index < textLength; ++index) {
				wchar_t realChar = bstrText[index];
				bool charAdded = false;
				if (realChar == OBJ_REPLACEMENT_CHAR) {
					const long charIndex = startOffset + index;
					long hyperlinkIndex = 0;
					if (paccHypertext && paccHypertext->get_hyperlinkIndex(charIndex, &hyperlinkIndex) == S_OK) {
						CComPtr<IAccessibleHyperlink> paccHyperlink;
						if (S_OK == paccHypertext->get_hyperlink(hyperlinkIndex, &paccHyperlink)) {
							CComQIPtr <IAccessible2> pacc2Child(paccHyperlink);
							if (pacc2Child) {
								map<wstring, wstring> childAttribsMap;
								fetchIA2Attributes(pacc2Child, childAttribsMap);
								auto liveItr = childAttribsMap.find(L"live");
								if (liveItr == childAttribsMap.end() || liveItr->second != L"off") {
									if (getTextFromIAccessible(textBuf, pacc2Child)) {
										gotText = true;
									}
								}
								charAdded = true;
							}
						}
					}
				}
				if (!charAdded && includeTopLevelText) {
					textBuf.append(1, realChar);
					charAdded = true;
					if (realChar != OBJ_REPLACEMENT_CHAR && !iswspace(realChar)) {
						gotText = true;
					}
				}
			}
			textBuf.append(1, L' ');
		}
	}
	if (!gotText && !useNewText) {
		gotText = appendNameDescription(pacc2, textBuf);
	}
	return gotText;
}
#endif
```

* [ ] **Step 2: Build the helper DLL**

From the repo root:

```sh
scons.bat source\lib\x64\nvdaHelperRemote.dll
```

Expected: clean build (no warnings; `/WX` is on so any warning fails the build). The link line should include both `build\rust\release\nvda_input_hooks.lib` and `build\rust\release\nvda_ia2.lib`.

* [ ] **Step 3: Confirm `nvda_ia2_get_text_from_iaccessible` is on the link**

```sh
dumpbin /symbols build\rust\release\nvda_ia2.lib | findstr nvda_ia2_get_text_from_iaccessible
```

Expected: at least one line mentioning `nvda_ia2_get_text_from_iaccessible` as a public external symbol.

* [ ] **Step 4: Confirm the DLL exports / size changed**

The DLL should grow modestly (a few KB) compared to the post-PR-1 baseline. No regression in pre-existing exports.

* [ ] **Step 5: Commit**

```sh
git add nvdaHelper/remote/textFromIAccessible.cpp
git commit -m "Delegate getTextFromIAccessible to Rust on x86_64"
```

---

## Task 6 (manual): Smoke-test in Firefox

This task is for the human operator after the agent reports Tasks 1-5 complete. Build is verified at the unit-test / link level by Tasks 3-5; smoke-test is the only integration gate.

* Run `runnvda.bat` to launch the dev build.
* Open Firefox and navigate to a structured page that exercises:
  * **Headings:** `H` browse-mode key — confirm heading levels are announced (verifies the IA2 attribute path that PR 1 covers, since `getTextFromIAccessible` calls `fetchIA2Attributes` for the `live` filter).
  * **Links inside paragraphs:** browse-mode line/word reading should announce link text correctly (this exercises the embedded-object-character → hyperlink-recursion code path).
  * **Aria-live=off regions:** these should be skipped by recursion (matches existing C++ behavior).
* Open NVDA's log viewer (`NVDA+F1`) and confirm there are no `panic` / `nvda_ia2` error entries.
* If any regression is observed, do not push — investigate first.

## Task 7 (manual): Push

Once the smoke test passes:

```sh
git push origin worktree-rust-beep-generator
```

Branch already tracks `origin/worktree-rust-beep-generator` from PR 1.
