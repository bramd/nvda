# Rust port of `ia2LiveRegions` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `nvdaHelper/remote/ia2LiveRegions.cpp` to Rust on x86_64. Non-x86_64 keeps the verbatim C++ under `#ifdef _M_X64`.

**Architecture:** Mirrors PR 2. The C++ `winEventProcHook` keeps doing its Win32-only setup (event-type filter, `AccessibleObjectFromEvent`, accState fetch, QI to `IAccessible2`); after that point it delegates to a single `extern "C"` Rust function that runs the IA2-attribute predicate chain, the `findAriaAtomic` walk, the background-tab check, the text retrieval, and reports back via callback. The C++ side adapts the callback into the existing `nvdaControllerInternal_reportLiveRegion` RPC call.

**Tech Stack:** Rust crate `nvda_ia2` (already established in PR 1+2), windows-rs 0.58, scons/MSVC for the C++ side.

Companion design doc: `docs/plans/2026-05-04-rust-ia2-live-regions-design.md`.

---

## Task 1: Add `IAccessible2::get_states` and `get_uniqueID` Rust method wrappers

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs`

The vtable slots for `get_states` (slot 8) and `get_uniqueID` (slot 14) were already declared in PR 1 as `usize` placeholders. Promote them to typed function pointers and add Rust wrappers. The two methods both have a `[out, retval]` `long*` and no other params.

* [ ] **Step 1: Promote the vtable slots to typed function pointers**

In `rust/nvda_ia2/src/interfaces.rs`, find the `IAccessible2_Vtbl` struct (around line 71). Replace:

```rust
    pub get_states: usize,
```

with:

```rust
    pub get_states: unsafe extern "system" fn(this: *mut core::ffi::c_void, states: *mut i32) -> HRESULT,
```

And replace:

```rust
    pub get_uniqueID: usize,
```

with:

```rust
    pub get_uniqueID: unsafe extern "system" fn(this: *mut core::ffi::c_void, unique_id: *mut i32) -> HRESULT,
```

Leave the rest of the struct unchanged.

* [ ] **Step 2: Add Rust wrappers**

In the same file, find the `impl IAccessible2 { ... }` block and append two methods to it (after `get_attributes`, before the closing `}`):

```rust
    /// Returns the IA2 state bitmask. See `include/ia2/api/AccessibleStates.idl`
    /// for flag definitions (`IA2_STATE_EDITABLE`, etc.).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessible2` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_states(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_states)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the IA2 unique ID for this object. See
    /// `include/ia2/api/Accessible2.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_uniqueID(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_uniqueID)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }
```

* [ ] **Step 3: Verify build, tests, and clippy**

From the worktree root:

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
```

Expected: clean build, all 27 existing tests pass, no clippy warnings.

* [ ] **Step 4: Commit**

```sh
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add Rust wrappers for IAccessible2::get_states and get_uniqueID"
```

---

## Task 2: Add `live_regions` module with pure attribute predicates and tests (TDD)

**Files:**

* Create: `rust/nvda_ia2/src/live_regions.rs`
* Modify: `rust/nvda_ia2/src/lib.rs` (add `pub mod live_regions;`)

The IA2-attribute predicate chain has a lot of pure logic. Pull it out as plain functions over `&BTreeMap<String, String>` with full unit tests. The COM-orchestration parts come in later tasks.

* [ ] **Step 1: Create the module file**

Create `rust/nvda_ia2/src/live_regions.rs` with:

```rust
//! Port of `nvdaHelper/remote/ia2LiveRegions.cpp`.
//!
//! For now this module exposes only the pure attribute predicates over
//! the IA2 attribute map. The COM-orchestration helpers
//! (`find_aria_atomic`, `is_in_background_tab`, the event handler, and
//! the `extern "C"` shim) are added in follow-up commits.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePoliteness {
    Polite,
    Assertive,
    Rude,
}

impl LivePoliteness {
    /// The `container-live` attribute value that yielded this politeness.
    /// The same string is forwarded to `nvdaControllerInternal_reportLiveRegion`.
    pub fn as_str(&self) -> &'static str {
        match self {
            LivePoliteness::Polite => "polite",
            LivePoliteness::Assertive => "assertive",
            LivePoliteness::Rude => "rude",
        }
    }
}

/// Read the `container-live` IA2 attribute and map it to a
/// [`LivePoliteness`] if the value is one the live-region hook
/// recognises. Mirrors the predicate at `ia2LiveRegions.cpp:147-148`.
pub fn parse_live_politeness(map: &BTreeMap<String, String>) -> Option<LivePoliteness> {
    match map.get("container-live")?.as_str() {
        "polite" => Some(LivePoliteness::Polite),
        "assertive" => Some(LivePoliteness::Assertive),
        "rude" => Some(LivePoliteness::Rude),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relevance {
    pub additions: bool,
    pub text: bool,
}

/// Read the `container-relevant` IA2 attribute. Mirrors the parsing at
/// `ia2LiveRegions.cpp:176-185`. Absent / `all` -> additions+text;
/// otherwise look for the words `additions` and `text`.
pub fn parse_container_relevant(map: &BTreeMap<String, String>) -> Relevance {
    match map.get("container-relevant") {
        None => Relevance { additions: true, text: true },
        Some(v) if v == "all" => Relevance { additions: true, text: true },
        Some(v) => Relevance {
            additions: v.contains("additions"),
            text: v.contains("text"),
        },
    }
}

/// Mirrors `ia2LiveRegions.cpp:171-172`.
pub fn is_container_busy(map: &BTreeMap<String, String>) -> bool {
    map.get("container-busy").map(|v| v == "true").unwrap_or(false)
}

/// Mirrors `ia2LiveRegions.cpp:31-32`.
pub fn is_atomic(map: &BTreeMap<String, String>) -> bool {
    map.get("atomic").map(|v| v == "true").unwrap_or(false)
}

/// Mirrors `ia2LiveRegions.cpp:38-39`.
pub fn is_container_atomic(map: &BTreeMap<String, String>) -> bool {
    map.get("container-atomic").map(|v| v == "true").unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn live_politeness_absent_is_none() {
        assert_eq!(parse_live_politeness(&map(&[])), None);
    }

    #[test]
    fn live_politeness_off_is_none() {
        assert_eq!(parse_live_politeness(&map(&[("container-live", "off")])), None);
    }

    #[test]
    fn live_politeness_polite() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "polite")])),
            Some(LivePoliteness::Polite),
        );
    }

    #[test]
    fn live_politeness_assertive() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "assertive")])),
            Some(LivePoliteness::Assertive),
        );
    }

    #[test]
    fn live_politeness_rude() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "rude")])),
            Some(LivePoliteness::Rude),
        );
    }

    #[test]
    fn live_politeness_unknown_is_none() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "loud")])),
            None,
        );
    }

    #[test]
    fn relevant_absent_defaults_to_all() {
        assert_eq!(
            parse_container_relevant(&map(&[])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_all_explicit() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "all")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_additions_only() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "additions")])),
            Relevance { additions: true, text: false },
        );
    }

    #[test]
    fn relevant_text_only() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "text")])),
            Relevance { additions: false, text: true },
        );
    }

    #[test]
    fn relevant_additions_and_text() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "additions text")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_text_and_additions() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "text additions")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_unrecognized_is_neither() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "removals")])),
            Relevance { additions: false, text: false },
        );
    }

    #[test]
    fn busy_true() {
        assert!(is_container_busy(&map(&[("container-busy", "true")])));
    }

    #[test]
    fn busy_false_value() {
        assert!(!is_container_busy(&map(&[("container-busy", "false")])));
    }

    #[test]
    fn busy_absent() {
        assert!(!is_container_busy(&map(&[])));
    }

    #[test]
    fn atomic_true() {
        assert!(is_atomic(&map(&[("atomic", "true")])));
    }

    #[test]
    fn atomic_absent() {
        assert!(!is_atomic(&map(&[])));
    }

    #[test]
    fn container_atomic_true() {
        assert!(is_container_atomic(&map(&[("container-atomic", "true")])));
    }

    #[test]
    fn container_atomic_absent() {
        assert!(!is_container_atomic(&map(&[])));
    }
}
```

* [ ] **Step 2: Add the module to the crate root**

Modify `rust/nvda_ia2/src/lib.rs`. Find:

```rust
pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod text;
pub mod types;
```

Insert `pub mod live_regions;` in alphabetical position (between `interfaces` and `text`):

```rust
pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod live_regions;
pub mod text;
pub mod types;
```

* [ ] **Step 3: Run the new tests, confirm they pass**

```sh
cargo test --manifest-path rust/nvda_ia2/Cargo.toml --lib live_regions::
```

Expected: 18 tests pass.

* [ ] **Step 4: Verify clippy stays clean**

```sh
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
```

Expected: no warnings.

* [ ] **Step 5: Commit**

```sh
git add rust/nvda_ia2/src/live_regions.rs rust/nvda_ia2/src/lib.rs
git commit -m "Add IA2 live-region attribute predicates with unit tests"
```

---

## Task 3: Port `find_aria_atomic`

**Files:**

* Modify: `rust/nvda_ia2/src/live_regions.rs` (append)

Recursive walk up `accParent` looking for an `atomic="true"` ancestor when the starting node has `container-atomic="true"`.

* [ ] **Step 1: Append the function**

Append to `rust/nvda_ia2/src/live_regions.rs`, AFTER the existing module-level functions and BEFORE the `#[cfg(test)]` block:

```rust
use crate::attribs::parse_attribs;
use crate::interfaces::IAccessible2;
use windows::core::Interface;

/// If `pacc2` declares `atomic="true"`, returns it (cloned, AddRef'd).
/// Otherwise, if it declares `container-atomic="true"`, walks up
/// `accParent` and returns the nearest atomic ancestor (recursively).
/// Returns `None` if no atomic ancestor exists.
///
/// Mirrors `findAriaAtomic` in `ia2LiveRegions.cpp:30-56`.
///
/// `attribs_map` is the IA2 attributes for `pacc2` -- the caller already
/// has these for the entry node, so we take them as a parameter rather
/// than fetching twice.
///
/// # Safety
///
/// `pacc2` must be a live, well-formed `IAccessible2` for the duration
/// of the call. The recursive walk dereferences each parent pointer the
/// COM server returns.
pub unsafe fn find_aria_atomic(
    pacc2: &IAccessible2,
    attribs_map: &BTreeMap<String, String>,
) -> Option<IAccessible2> {
    if is_atomic(attribs_map) {
        return Some(pacc2.clone());
    }
    if !is_container_atomic(attribs_map) {
        return None;
    }
    // Walk up to the parent. accParent returns IDispatch; QI to
    // IAccessible2.
    let parent_disp = unsafe { pacc2.accParent() }.ok()?;
    let parent_acc2: IAccessible2 = parent_disp.cast().ok()?;
    let parent_bstr = unsafe { parent_acc2.get_attributes() }.ok()?;
    // BSTR -> String works for both null and zero-length BSTRs (both
    // produce ""), and parse_attribs("") returns an empty map. The
    // recursion bails on that empty map at the next is_container_atomic
    // check.
    let parent_map = parse_attribs(&parent_bstr.to_string());
    unsafe { find_aria_atomic(&parent_acc2, &parent_map) }
}
```

* [ ] **Step 2: Verify build and clippy**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
```

Expected: clean build, no clippy warnings, 45 tests pass (27 existing + 18 from Task 2).

* [ ] **Step 3: Commit**

```sh
git add rust/nvda_ia2/src/live_regions.rs
git commit -m "Port findAriaAtomic to Rust"
```

---

## Task 4: Port `is_in_background_tab` and `ia2_unique_id_from_dispatch_variant`

**Files:**

* Modify: `rust/nvda_ia2/src/live_regions.rs` (append)

The background-tab check uses two `accNavigate` calls (one on the event target via `NAVRELATION_CONTAINING_TAB_PANE`, one on the window root via `NAVRELATION_EMBEDS`) and compares the returned IA2 unique IDs. If they differ, the event target is in a background tab.

* [ ] **Step 1: Append the helpers**

Append to `rust/nvda_ia2/src/live_regions.rs`, after `find_aria_atomic`:

```rust
use windows::core::VARIANT;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{IDispatch, IServiceProvider};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};

/// IA2 navigation relations. See `include/ia2/api/IA2Relations.idl`. These
/// constants are not exported by windows-rs, so we declare them locally.
pub(crate) const NAVRELATION_EMBEDS: i32 = 0x1009;
pub(crate) const NAVRELATION_CONTAINING_TAB_PANE: i32 = 0x1012;

/// `OBJID_CLIENT` per oleacc.h. The windows-rs constant lives in
/// `Win32_UI_WindowsAndMessaging` (a feature we don't currently enable);
/// declare locally to avoid pulling in the whole module surface.
pub(crate) const OBJID_CLIENT: u32 = 0xFFFF_FFFC; // -4 as u32

/// Pull the IA2 `uniqueID` out of a VARIANT that should hold an IDispatch
/// pointing to an `IAccessible`. Mirrors `getIa2UniqueIdFromDispatchVariant`
/// in `ia2LiveRegions.cpp:58-74`. Returns `0` for any failure path
/// (matches the C++ contract; the caller compares against another id and
/// `0` falls through to "unknown" treatment).
pub fn ia2_unique_id_from_dispatch_variant(variant: &VARIANT) -> i32 {
    let Ok(disp) = IDispatch::try_from(variant) else { return 0 };
    let Ok(serv) = disp.cast::<IServiceProvider>() else { return 0 };
    // SAFETY: QueryService is FFI; we hold a live IServiceProvider via
    // `serv` for the duration of the call.
    let Ok(acc2) = (unsafe { serv.QueryService::<IAccessible2>(&IAccessible::IID) })
    else {
        return 0;
    };
    // SAFETY: acc2 is a live IAccessible2 we just received from QueryService.
    unsafe { acc2.get_uniqueID() }.unwrap_or(0)
}

/// Returns `true` if `pacc2` lives in a Firefox background tab. Mirrors
/// `isInBackgroundTab` in `ia2LiveRegions.cpp:76-107`.
///
/// In Firefox, all tabs share the same HWND. The "containing tab pane"
/// for the event target is compared against the "embedded" tab pane on
/// the window root: if they have different IA2 unique IDs, the event
/// target is in a background tab.
///
/// # Safety
///
/// `pacc2` must be a live `IAccessible2`; `hwnd` must be a valid window
/// handle for the duration of the call.
pub unsafe fn is_in_background_tab(pacc2: &IAccessible2, hwnd: HWND) -> bool {
    let pacc: &IAccessible = pacc2;
    let start = VARIANT::from(0i32); // CHILDID_SELF
    let acc_doc = match unsafe { pacc.accNavigate(NAVRELATION_CONTAINING_TAB_PANE, &start) } {
        Ok(v) => v,
        Err(_) => return false,
    };
    let acc_doc_id = ia2_unique_id_from_dispatch_variant(&acc_doc);
    if acc_doc_id == 0 {
        return false;
    }
    // Get the root accessible for the window.
    let mut root_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
    if unsafe {
        AccessibleObjectFromWindow(
            hwnd,
            OBJID_CLIENT,
            &IAccessible::IID,
            &mut root_ptr,
        )
    }
    .is_err()
    {
        return false;
    }
    if root_ptr.is_null() {
        return false;
    }
    // Take ownership of the AddRef'd IAccessible the out-param contract
    // gave us. `from_raw` consumes the raw pointer's reference; `root`'s
    // Drop (i.e. `Release`) balances it. Mirrors the C++ CComPtr.
    let root: IAccessible = unsafe { IAccessible::from_raw(root_ptr) };
    let fg_doc = match unsafe { root.accNavigate(NAVRELATION_EMBEDS, &start) } {
        Ok(v) => v,
        Err(_) => return false,
    };
    let fg_doc_id = ia2_unique_id_from_dispatch_variant(&fg_doc);
    if fg_doc_id == 0 {
        return false;
    }
    acc_doc_id != fg_doc_id
}
```

The `IAccessible::from_raw_borrowed(&root_ptr)` followed by `.clone()` is the windows-rs pattern for taking ownership of an out-param `*mut c_void` that we don't want to leak: the borrow-clone pair AddRefs once (so we own the new ref), and the original out-param ref is intentionally leaked because `AccessibleObjectFromWindow` returned it as "raw owned." We then balance by `Release` on drop. This matches the lifetime contract `CComPtr<IAccessible>` had in the C++.

* [ ] **Step 2: Verify build / clippy / tests**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
```

Expected: clean build, no warnings, 45 tests still pass.

If clippy flags `OBJID_CLIENT` not being a `u32` -- it's typically declared as `OBJECT_IDENTIFIER` in windows-rs which is a thin wrapper. The `.0 as u32` cast is the standard escape hatch.

If `AccessibleObjectFromWindow` complains that `&IAccessible::IID` is the wrong pointer type, it expects `*const GUID`; coerce with `&IAccessible::IID as *const _`.

* [ ] **Step 3: Commit**

```sh
git add rust/nvda_ia2/src/live_regions.rs
git commit -m "Port isInBackgroundTab and dispatch-variant uniqueID helper"
```

---

## Task 5: Add the event-handler glue and `extern "C"` shim

**Files:**

* Modify: `rust/nvda_ia2/src/live_regions.rs` (append)

This task adds the function that runs the full filter chain after the C++ side has done the Win32-only setup, plus the `extern "C"` shim and its `ReportLiveRegionCallback` typedef.

* [ ] **Step 1: Append the handler and shim**

Append to `rust/nvda_ia2/src/live_regions.rs`, after `is_in_background_tab`:

```rust
use crate::text::get_text_from_iaccessible_collect;
use crate::types::IA2_STATE_EDITABLE;

/// `STATE_SYSTEM_OFFSCREEN` per oleacc.h. The windows-rs constant lives
/// in `Win32_UI_Controls` (a feature we don't currently enable); declare
/// locally to avoid pulling in the whole module surface.
const STATE_SYSTEM_OFFSCREEN: i32 = 0x10000;

/// WinEvent identifiers the live-region hook cares about. Pre-filtered by
/// the C++ side so Rust doesn't need to import the platform constants.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveRegionEvent {
    NameChange = 0,
    DescriptionChange = 1,
    Show = 2,
    TextInserted = 3,
    TextUpdated = 4,
}

impl LiveRegionEvent {
    fn from_raw(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::NameChange),
            1 => Some(Self::DescriptionChange),
            2 => Some(Self::Show),
            3 => Some(Self::TextInserted),
            4 => Some(Self::TextUpdated),
            _ => None,
        }
    }
}

/// C-callable callback invoked once at the end of
/// [`nvda_ia2_handle_live_region_event`] when the event passes all filters.
/// Mirrors the AttribCallback / AppendCharsCallback pattern from earlier
/// FFI shims.
///
/// # Safety
///
/// The callback must not unwind. Both pointers are valid for their
/// respective `_len` `u16` elements; the callback must copy the data
/// before returning.
pub type ReportLiveRegionCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    text_ptr: *const u16,
    text_len: usize,
    politeness_ptr: *const u16,
    politeness_len: usize,
);

/// C-callable replacement for the IA2 portion of `winEventProcHook`.
///
/// `pacc2` is borrowed (no `Release`). `event_kind` is one of the
/// [`LiveRegionEvent`] discriminants; any other value yields `false`
/// without invoking the callback. `acc_state` is the IA2 state bitmask
/// the C++ side fetched (passes `0` if the source VARIANT was not
/// `VT_I4`). `report_cb` is invoked at most once, only when the event
/// passes every filter.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `hwnd` must be a valid `HWND` (or null; the background-tab check
///   bails on a null window).
/// * `report_cb` must be a valid function pointer; `ctx` is opaque user
///   data. `report_cb` must not unwind.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_handle_live_region_event(
    pacc2: *mut core::ffi::c_void,
    hwnd: *mut core::ffi::c_void,
    event_kind: u32,
    acc_state: i32,
    ctx: *mut core::ffi::c_void,
    report_cb: ReportLiveRegionCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let event = match LiveRegionEvent::from_raw(event_kind) {
        Some(e) => e,
        None => return false,
    };
    let hwnd = HWND(hwnd);
    handle_live_region_event(acc2, hwnd, event, acc_state, ctx, report_cb)
}

unsafe fn handle_live_region_event(
    pacc2: &IAccessible2,
    hwnd: HWND,
    event: LiveRegionEvent,
    acc_state: i32,
    ctx: *mut core::ffi::c_void,
    report_cb: ReportLiveRegionCallback,
) -> bool {
    // Fetch IA2 attributes; bail if unavailable.
    let attribs_bstr = unsafe { pacc2.get_attributes() }.ok();
    let attribs_map: BTreeMap<String, String> = match attribs_bstr {
        Some(b) => parse_attribs(&b.to_string()),
        None => return false,
    };

    // container-live filter.
    let politeness = match parse_live_politeness(&attribs_map) {
        Some(p) => p,
        None => return false,
    };

    // Background-tab filter (only when the offscreen state bit is set).
    if (acc_state & STATE_SYSTEM_OFFSCREEN) != 0
        && unsafe { is_in_background_tab(pacc2, hwnd) }
    {
        return false;
    }

    // IA2_STATE_EDITABLE filter -- editable text should never announce
    // as a live region (typed characters would be echoed twice, etc.).
    let ia2_states = unsafe { pacc2.get_states() }.unwrap_or(0);
    if (ia2_states & IA2_STATE_EDITABLE) != 0 {
        return false;
    }

    // container-busy filter.
    if is_container_busy(&attribs_map) {
        return false;
    }

    // container-relevant parse.
    let relevance = parse_container_relevant(&attribs_map);
    if !relevance.additions && !relevance.text {
        return false;
    }

    // Show events only flow through if additions are allowed.
    if event == LiveRegionEvent::Show && !relevance.additions {
        return false;
    }

    // Show edge case: ignore the event if there's a parent we can text
    // through OR the parent has its own valid container-live (we are
    // not the root of the region).
    if event == LiveRegionEvent::Show && should_ignore_show_event(pacc2) {
        return false;
    }

    // Name/description changes only flow through if text is allowed.
    if !relevance.text
        && (event == LiveRegionEvent::NameChange
            || event == LiveRegionEvent::DescriptionChange)
    {
        return false;
    }

    // Resolve the text.
    let mut text_buf: Vec<u16> = Vec::new();
    let pacc2_atomic = unsafe { find_aria_atomic(pacc2, &attribs_map) };
    let got_text = if let Some(atomic) = pacc2_atomic.as_ref() {
        get_text_from_iaccessible_collect(&mut text_buf, atomic, false, true, true)
    } else {
        match event {
            LiveRegionEvent::NameChange => {
                let varchild = VARIANT::from(0i32);
                let pacc: &IAccessible = pacc2;
                if let Ok(name) = unsafe { pacc.get_accName(&varchild) } {
                    text_buf.extend_from_slice(name.as_wide());
                    true
                } else {
                    false
                }
            }
            LiveRegionEvent::DescriptionChange => {
                let varchild = VARIANT::from(0i32);
                let pacc: &IAccessible = pacc2;
                if let Ok(desc) = unsafe { pacc.get_accDescription(&varchild) } {
                    text_buf.extend_from_slice(desc.as_wide());
                    true
                } else {
                    false
                }
            }
            LiveRegionEvent::Show => get_text_from_iaccessible_collect(
                &mut text_buf, pacc2, false, true, true,
            ),
            LiveRegionEvent::TextInserted | LiveRegionEvent::TextUpdated => {
                get_text_from_iaccessible_collect(
                    &mut text_buf,
                    pacc2,
                    true,
                    relevance.additions,
                    relevance.text,
                )
            }
        }
    };

    if !got_text || text_buf.is_empty() {
        return false;
    }

    let politeness_str = politeness.as_str();
    let politeness_utf16: Vec<u16> = politeness_str.encode_utf16().collect();
    unsafe {
        report_cb(
            ctx,
            text_buf.as_ptr(),
            text_buf.len(),
            politeness_utf16.as_ptr(),
            politeness_utf16.len(),
        );
    }
    true
}

unsafe fn should_ignore_show_event(pacc2: &IAccessible2) -> bool {
    let pacc: &IAccessible = pacc2;
    let parent_disp = match unsafe { pacc.accParent() } {
        Ok(d) => d,
        Err(_) => return false,
    };
    // If the parent has IAccessibleText, the upcoming text events handle
    // this better -- ignore the show event.
    if parent_disp.cast::<crate::interfaces::IAccessibleText>().is_ok() {
        return true;
    }
    // Otherwise, default to "we are the root, do not ignore" unless the
    // parent has a valid container-live, in which case we are not the
    // root.
    let parent_acc2: IAccessible2 = match parent_disp.cast() {
        Ok(a) => a,
        Err(_) => return true, // No IA2 on parent -> assume we are root
    };
    let parent_bstr = match unsafe { parent_acc2.get_attributes() } {
        Ok(b) => b,
        Err(_) => return true,
    };
    let parent_map = parse_attribs(&parent_bstr.to_string());
    parse_live_politeness(&parent_map).is_none()
}
```

Note: this task references two items that need to exist before it compiles:

1. `crate::types::IA2_STATE_EDITABLE` -- declare it in `rust/nvda_ia2/src/types.rs` as `pub const IA2_STATE_EDITABLE: i32 = 0x8;` if not already present.
2. `crate::text::get_text_from_iaccessible_collect` -- this is currently `get_text_from_iaccessible` in `text.rs`, but it's `fn` (private). Promote it to `pub(crate)` and rename to `get_text_from_iaccessible_collect` so the live-regions code can call it directly without going through the FFI shim.

* [ ] **Step 2: Add the IA2_STATE_EDITABLE constant**

In `rust/nvda_ia2/src/types.rs`, append (or place alongside other IA2 constants if any exist):

```rust
/// `IA2_STATE_EDITABLE` per `include/ia2/api/AccessibleStates.idl:101`.
pub const IA2_STATE_EDITABLE: i32 = 0x8;
```

* [ ] **Step 3: Promote `get_text_from_iaccessible` for in-crate callers**

In `rust/nvda_ia2/src/text.rs`, find:

```rust
fn get_text_from_iaccessible(
    text_buf: &mut Vec<u16>,
    pacc2: &IAccessible2,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
) -> bool {
```

Rename to `get_text_from_iaccessible_collect` and make it `pub(crate)`:

```rust
pub(crate) fn get_text_from_iaccessible_collect(
    text_buf: &mut Vec<u16>,
    pacc2: &IAccessible2,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
) -> bool {
```

Update the call site inside the same file (`nvda_ia2_get_text_from_iaccessible`) to use the new name.

* [ ] **Step 4: Verify build / clippy / tests**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
```

Expected: clean build, no warnings, 45 tests still pass.

If clippy complains about the `name.as_wide()` / `desc.as_wide()` unused-binding pattern when get_accName returns Err -- the if-let guards already gate that, no fix needed unless clippy gets specifically loud.

If `HWND(hwnd)` doesn't compile because `HWND` is a tuple struct in windows-rs 0.58, the spelling may need to be `HWND(hwnd as _)` or `HWND::default()`. Check the actual definition in `windows::Win32::Foundation::HWND` if so.

* [ ] **Step 5: Commit**

```sh
git add rust/nvda_ia2/src/live_regions.rs rust/nvda_ia2/src/text.rs rust/nvda_ia2/src/types.rs
git commit -m "Add live-region event handler and extern C shim"
```

---

## Task 6: Wire the C++ delegation in `ia2LiveRegions.cpp`

**Files:**

* Modify: `nvdaHelper/remote/ia2LiveRegions.cpp`

The C++ retains the Win32-only setup (event-type filter, visibility filter, `AccessibleObjectFromEvent`, accState fetch, `STATE_SYSTEM_INVISIBLE` early-return, QI to `IAccessible2`). After that point, on x86_64 it builds the event-kind tag and calls the Rust shim. Non-x86_64 keeps the original full implementation under `#else`.

* [ ] **Step 1: Replace the contents of `nvdaHelper/remote/ia2LiveRegions.cpp`**

Use the Write tool to overwrite the file with:

```cpp
/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2006-2021 NV Access Limited, Google LLC, Leonard de Ruijter
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include <string>
#include <sstream>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <atlcomcli.h>
#include <remote/nvdaControllerInternal.h>
#include <common/ia2utils.h>
#include "nvdaHelperRemote.h"
#include "textFromIAccessible.h"

using namespace std;

#ifdef _M_X64
extern "C" {
	typedef void (*ReportLiveRegionCallback)(
		void* ctx,
		const wchar_t* text_ptr,     size_t text_len,
		const wchar_t* polite_ptr,   size_t polite_len);

	bool nvda_ia2_handle_live_region_event(
		void* pacc2,
		void* hwnd,
		unsigned int event_kind,
		int acc_state,
		void* ctx,
		ReportLiveRegionCallback report_cb);
}

namespace {
	void report_live(void* /*ctx*/,
	                 const wchar_t* text_ptr,   size_t text_len,
	                 const wchar_t* polite_ptr, size_t polite_len) {
		try {
			std::wstring text(text_ptr, text_len);
			std::wstring polite(polite_ptr, polite_len);
			nvdaControllerInternal_reportLiveRegion(text.c_str(), polite.c_str());
		} catch (const std::bad_alloc&) {
			// Suppressed to prevent UB from a C++ exception crossing the
			// extern "C" frame back into Rust.
		}
	}
}

void CALLBACK winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	HWND fgHwnd = GetForegroundWindow();
	if (!IsWindowVisible(hwnd) || (hwnd != fgHwnd && !IsChild(fgHwnd, hwnd))) return;

	unsigned int eventKind;
	switch (eventID) {
		case EVENT_OBJECT_NAMECHANGE:        eventKind = 0; break;
		case EVENT_OBJECT_DESCRIPTIONCHANGE: eventKind = 1; break;
		case EVENT_OBJECT_SHOW:              eventKind = 2; break;
		case IA2_EVENT_TEXT_INSERTED:        eventKind = 3; break;
		case IA2_EVENT_TEXT_UPDATED:         eventKind = 4; break;
		default: return;
	}

	CComPtr<IAccessible> pacc;
	CComVariant varChild;
	if (AccessibleObjectFromEvent(hwnd, objectID, childID, &pacc, &varChild) != S_OK) {
		return;
	}

	CComVariant varState;
	pacc->get_accState(varChild, &varState);
	if (varState.vt == VT_I4 && (varState.lVal & STATE_SYSTEM_INVISIBLE)) {
		return;
	}
	int accState = (varState.vt == VT_I4) ? varState.lVal : 0;

	CComQIPtr<IServiceProvider> pserv(pacc);
	if (!pserv) return;
	CComPtr<IAccessible2> pacc2;
	pserv->QueryService(IID_IAccessible, IID_IAccessible2, (void**)(&pacc2));
	if (!pacc2) return;

	nvda_ia2_handle_live_region_event(
		pacc2, hwnd, eventKind, accState,
		nullptr, report_live);
}

#else
// Non-x86_64 fallback: keep the original C++ implementation because cargo
// only produces a host-triple staticlib. Same code as before this PR,
// kept verbatim. Multi-arch cargo builds are a future exercise.

const long NAVRELATION_EMBEDS = 0x1009;
const long NAVRELATION_CONTAINING_TAB_PANE = 0x1012;

IAccessible2* findAriaAtomic(IAccessible2* pacc2,map<wstring,wstring>& attribsMap) {
	map<wstring,wstring>::iterator i=attribsMap.find(L"atomic");
	bool atomic=(i!=attribsMap.end()&&i->second.compare(L"true")==0);
	IAccessible2* pacc2Atomic=NULL;
	if(atomic) {
		pacc2Atomic=pacc2;
		pacc2Atomic->AddRef();
	} else {
		i=attribsMap.find(L"container-atomic");
		if(i!=attribsMap.end()&&i->second.compare(L"true")==0) {
			IDispatch* pdispParent=NULL;
			pacc2->get_accParent(&pdispParent);
			if(pdispParent) {
				IAccessible2* pacc2Parent=NULL;
				if(pdispParent->QueryInterface(IID_IAccessible2,(void**)&pacc2Parent)==S_OK&&pacc2Parent) {
					map<wstring,wstring> parentAttribsMap;
					if(fetchIA2Attributes(pacc2Parent,parentAttribsMap)) {
						pacc2Atomic=findAriaAtomic(pacc2Parent,parentAttribsMap);
					}
					pacc2Parent->Release();
				}
				pdispParent->Release();
			}
		}
	}
	return pacc2Atomic;
}

long getIa2UniqueIdFromDispatchVariant(VARIANT& variant) {
	if (variant.vt != VT_DISPATCH || !variant.pdispVal) {
		return 0;
	}
	CComQIPtr<IServiceProvider> serv = variant.pdispVal;
	if (!serv) {
		return 0;
	}
	CComPtr<IAccessible2> acc;
	serv->QueryService(IID_IAccessible, IID_IAccessible2, (void**)&acc);
	if (!acc) {
		return 0;
	}
	long id = 0;
	acc->get_uniqueID(&id);
	return id;
}

bool isInBackgroundTab(IAccessible* acc, HWND hwnd) {
	CComVariant start(0, VT_I4);
	CComVariant accDoc;
	HRESULT hr = acc->accNavigate(NAVRELATION_CONTAINING_TAB_PANE, start, &accDoc);
	if (FAILED(hr)) {
		return false;
	}
	long accDocId = getIa2UniqueIdFromDispatchVariant(accDoc);
	if (!accDocId) {
		return false;
	}
	CComPtr<IAccessible> root;
	AccessibleObjectFromWindow(hwnd, OBJID_CLIENT, IID_IAccessible, (void**)&root);
	if (!root) {
		return false;
	}
	CComVariant fgDoc;
	hr = root->accNavigate(NAVRELATION_EMBEDS, start, &fgDoc);
	if (FAILED(hr)) {
		return false;
	}
	long fgDocId = getIa2UniqueIdFromDispatchVariant(fgDoc);
	if (!fgDocId) {
		return false;
	}
	return accDocId != fgDocId;
}

void CALLBACK winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	HWND fgHwnd=GetForegroundWindow();
	//Ignore events for windows that are invisible or are not in the foreground
	if(!IsWindowVisible(hwnd)||(hwnd!=fgHwnd&&!IsChild(fgHwnd,hwnd))) return;
	//Ignore all events but a few types
	switch(eventID) {
		case EVENT_OBJECT_NAMECHANGE:
		case EVENT_OBJECT_DESCRIPTIONCHANGE:
		case EVENT_OBJECT_SHOW:
		case IA2_EVENT_TEXT_UPDATED:
		case IA2_EVENT_TEXT_INSERTED:
		break;
		default:
		return;
	}
	CComPtr<IAccessible> pacc;
	CComVariant varChild;
	//Try getting the IAccessible from the event
	if(AccessibleObjectFromEvent(hwnd,objectID,childID,&pacc,&varChild)!=S_OK) {
		return;
	}
	//Retreave the object states, and if its invisible or offscreen ignore the event.
	CComVariant varState;
	pacc->get_accState(varChild,&varState);
	if(varState.vt==VT_I4&&(varState.lVal&STATE_SYSTEM_INVISIBLE)) {
		return;
	}
	//Retreave an IAccessible2 via IServiceProvider if it exists.
	CComQIPtr<IServiceProvider> pserv(pacc);
	if(!pserv) return;
	CComPtr<IAccessible2> pacc2;
	pserv->QueryService(IID_IAccessible, IID_IAccessible2, (void**)(&pacc2));
	if(!pacc2) return;
	//Retreave the IAccessible2 attributes, and if the object is not a live region then ignore the event.
	map<wstring,wstring> attribsMap;
	if(!fetchIA2Attributes(pacc2,attribsMap)) {
		return;
	}
	auto i=attribsMap.find(L"container-live");
	bool live=(i!=attribsMap.end()&&(i->second.compare(L"polite")==0||i->second.compare(L"assertive")==0||i->second.compare(L"rude")==0));
	if(!live) {
		return;
	}
	// #1318: In Firefox, all tabs have the same HWND. Objects in background
	// tabs do get the offscreen state, but offscreen live regions are used to
	// report visually hidden information, so we can't filter based on that.
	// Therefore, if the offscreen state is set, we do an additional background
	// check.
	if (varState.vt==VT_I4 && varState.lVal & STATE_SYSTEM_OFFSCREEN
			&& isInBackgroundTab(pacc2, hwnd)) {
		return;
	}
	long ia2States = 0;
	pacc2->get_states(&ia2States);
	if (ia2States & IA2_STATE_EDITABLE) {
		// This is editable text. Editable text should never be a live region, as
		// this causes typed characters to be echoed when they shouldn't, etc.
		// Nevertheless, some authors misguidedly set aria-live on editable text.
		// We explicitly ignore this here.
		return;
	}
	wstring politeness = i->second;
	i=attribsMap.find(L"container-busy");
	bool busy=(i!=attribsMap.end()&&i->second.compare(L"true")==0);
	if(busy) {
		return;
	}
	i=attribsMap.find(L"container-relevant");
	bool allowAdditions=false;
	bool allowText=false;
	//If relevant is not specifyed we will default to additions and text, if all is specified then we also use additions and text
	if(i==attribsMap.end()||i->second.compare(L"all")==0) {
		allowText=allowAdditions=true;
	} else { //we support additions if its specified, we support text if its specified
		allowText=(i->second.find(L"text",0)!=wstring::npos);
		allowAdditions=(i->second.find(L"additions",0)!=wstring::npos);
	}
	// We only support additions or text
	if(!allowAdditions&&!allowText) {
		return;
	}
	//Only handle show events if additions are allowed
	if(eventID==EVENT_OBJECT_SHOW&&!allowAdditions) {
		return;
	}
	// If this is a show event and this is not the root of the region and there is a text parent,
	// We can ignore this event as there will be text events which can handle this better
	if(eventID==EVENT_OBJECT_SHOW) {
		bool ignoreShowEvent=false;
		CComPtr<IDispatch> pdispParent;
		pacc2->get_accParent(&pdispParent);
		if(pdispParent) {
			// check for text on parent
			CComQIPtr<IAccessibleText> paccTextParent(pdispParent);
			if (paccTextParent) {
				ignoreShowEvent=true;
			}
			if(!ignoreShowEvent) {
				// Check for useful container-live on parent, as if missing or off, then child must be the root
				// Firstly, we assume we are the root of the region and therefore should ignore the event
				ignoreShowEvent=true;
				CComQIPtr<IAccessible2> pacc2Parent(pdispParent);
				if (pacc2Parent) {
					map<wstring,wstring> parentAttribsMap;
					if(fetchIA2Attributes(pacc2Parent,parentAttribsMap)) {
						i=parentAttribsMap.find(L"container-live");
						if(i!=parentAttribsMap.end()&&(i->second.compare(L"polite")==0||i->second.compare(L"assertive")==0||i->second.compare(L"rude")==0)) {
							// There is a valid container-live that is not off, so therefore the child is definitly not the root
							ignoreShowEvent=false;
						}
					}
				}
			}
		}
		if(ignoreShowEvent) {
			return;
		}
	}
	// name and description changes can only be announced if relevant is text
	if(!allowText&&(eventID==EVENT_OBJECT_NAMECHANGE||eventID==EVENT_OBJECT_DESCRIPTIONCHANGE)) {
		return;
	}
	wstring textBuf;
	bool gotText=false;
	CComPtr<IAccessible2> pacc2Atomic = findAriaAtomic(pacc2,attribsMap);
	if(pacc2Atomic) {
		gotText=getTextFromIAccessible(textBuf,pacc2Atomic);
	} else if(eventID==EVENT_OBJECT_NAMECHANGE) {
		CComBSTR name;
		CComVariant varChild(0, VT_I4);
		pacc2->get_accName(varChild,&name);
		if(name) {
			textBuf.append(name);
			gotText=true;
		}
	} else if(eventID==EVENT_OBJECT_DESCRIPTIONCHANGE) {
		CComBSTR desc;
		CComVariant varChild(0, VT_I4);
		pacc2->get_accDescription(varChild,&desc);
		if(desc) {
			textBuf.append(desc);
			gotText=true;
		}
	} else if(eventID==EVENT_OBJECT_SHOW) {
		gotText=getTextFromIAccessible(textBuf,pacc2);
	} else if(eventID==IA2_EVENT_TEXT_INSERTED||eventID==IA2_EVENT_TEXT_UPDATED) {
		gotText=getTextFromIAccessible(textBuf,pacc2,true,allowAdditions,allowText);
	}
	if (gotText && !textBuf.empty()) {
		nvdaControllerInternal_reportLiveRegion(textBuf.c_str(), politeness.c_str());
	}
}
#endif

void ia2LiveRegions_inProcess_initialize() {
	registerWinEventHook(winEventProcHook);
}

void ia2LiveRegions_inProcess_terminate() {
	unregisterWinEventHook(winEventProcHook);
}
```

Note: `winEventProcHook` is declared inside both `#ifdef _M_X64` and `#else` blocks, with the same signature; the `_initialize` / `_terminate` functions outside the conditional use whichever is in scope.

* [ ] **Step 2: Build the helper DLL**

```sh
scons.bat source\lib\x64\nvdaHelperRemote.dll
```

(Use a long Bash timeout, ~600000 ms.) Expected: clean build, no warnings (`/WX` is on). The link line should already include `propsys.lib` from the PR 2 SCons changes -- no further build-system tweaks should be needed.

If the link surfaces new unresolved imports from other windows-rs compilation units, do NOT modify SCons. Stop and report what's missing.

* [ ] **Step 3: Commit**

```sh
git add nvdaHelper/remote/ia2LiveRegions.cpp
git commit -m "Delegate ia2LiveRegions winEvent hook to Rust on x86_64"
```

---

## Task 7 (manual): Smoke-test in Firefox

After the agent reports Tasks 1-6 complete, the human operator verifies in Firefox:

* Run `runnvda.bat` to launch the dev build.
* Open Firefox and navigate to a page exercising live regions. A simple test page (or any site with a chat widget / status bar / toast) is fine. Confirm:
  * **`aria-live="polite"`** updates announce after the current speech.
  * **`aria-live="assertive"`** updates interrupt and announce immediately.
  * **`aria-live="off"`** updates are silent.
  * **`aria-busy="true"`** ancestor suppresses announcements; clearing it lets the next update through.
  * **`aria-atomic="true"`** announces the full region rather than just the changed text.
  * Background-tab updates are silent (open the live-region page in a second tab, focus a different tab, trigger updates).
* Open NVDA's log viewer (`NVDA+F1`) and confirm there are no `panic` or `nvda_ia2` error entries.
* If any regression is observed, do not push -- investigate first.

## Task 8 (manual): Push

Once smoke-test passes:

```sh
git push origin worktree-rust-beep-generator
```

Branch already tracks `origin/worktree-rust-beep-generator`.
