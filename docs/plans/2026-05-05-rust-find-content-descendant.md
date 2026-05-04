# Rust port of `findContentDescendant` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `findContentDescendant` from `nvdaHelper/remote/IA2Support.cpp` to Rust on x86_64. Non-x86_64 keeps the verbatim C++ under `#ifdef _M_X64`.

**Architecture:** Mirrors PR 2 / PR 3. The Rust port lives in a new `find_descendant` module in the existing `nvda_ia2` crate, exposed via an `extern "C"` shim. The C++ `findContentDescendant` body becomes a thin wrapper that calls into the Rust shim. Surrounding RPC / threading / Win32 hook plumbing stays C++.

**Tech Stack:** Rust crate `nvda_ia2`, windows-rs 0.58, scons/MSVC.

Companion design doc: `docs/plans/2026-05-05-rust-find-content-descendant-design.md`.

---

## Task 1: Add `IAccessibleText` wrappers for caret/selection/character-count

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs`

The vtable slots for `get_caretOffset` (slot 3), `get_nSelections` (slot 5), `get_selection` (slot 7), and `get_nCharacters` (slot 15) are already declared as `usize` placeholders. Promote them to typed function pointers and add Rust wrappers, mirroring the existing `get_text` / `get_newText` wrappers.

* [ ] **Step 1: Promote the four vtable slots to typed function pointers**

In `rust/nvda_ia2/src/interfaces.rs`, find the `IAccessibleText_Vtbl` struct. Replace these four lines:

```rust
    pub get_caretOffset: usize,
```

->

```rust
    pub get_caretOffset: unsafe extern "system" fn(this: *mut core::ffi::c_void, offset: *mut i32) -> HRESULT,
```

```rust
    pub get_nSelections: usize,
```

->

```rust
    pub get_nSelections: unsafe extern "system" fn(this: *mut core::ffi::c_void, n_selections: *mut i32) -> HRESULT,
```

```rust
    pub get_selection: usize,
```

->

```rust
    pub get_selection: unsafe extern "system" fn(this: *mut core::ffi::c_void, selection_index: i32, start_offset: *mut i32, end_offset: *mut i32) -> HRESULT,
```

```rust
    pub get_nCharacters: usize,
```

->

```rust
    pub get_nCharacters: unsafe extern "system" fn(this: *mut core::ffi::c_void, n_characters: *mut i32) -> HRESULT,
```

Leave the rest of the struct unchanged.

* [ ] **Step 2: Add the four wrappers to `impl IAccessibleText`**

Append to the end of the existing `impl IAccessibleText { ... }` block (the one with `get_text` and `get_newText`):

```rust
    /// Returns the current caret offset within this text. See
    /// `include/ia2/api/AccessibleText.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_caretOffset(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_caretOffset)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the total character count of this text.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nCharacters(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nCharacters)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the number of active text selections.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nSelections(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nSelections)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the `(startOffset, endOffset)` for the selection at
    /// `selection_index`.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_selection(
        &self,
        selection_index: i32,
    ) -> windows::core::Result<(i32, i32)> {
        let mut start: i32 = 0;
        let mut end: i32 = 0;
        let hr = (Interface::vtable(self).get_selection)(
            Interface::as_raw(self),
            selection_index,
            &mut start as *mut _,
            &mut end as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((start, end))
    }
```

* [ ] **Step 3: Verify build, tests, clippy**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
```

Expected: clean build, all 47 existing tests pass, no clippy warnings.

* [ ] **Step 4: Commit**

```sh
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add Rust wrappers for IAccessibleText caret, selection, and character-count getters"
```

---

## Task 2: Implement `find_content_descendant` and `extern "C"` shim

**Files:**

* Create: `rust/nvda_ia2/src/find_descendant.rs`
* Modify: `rust/nvda_ia2/src/lib.rs` (add `pub mod find_descendant;`)

* [ ] **Step 1: Create `rust/nvda_ia2/src/find_descendant.rs`**

```rust
//! Port of `findContentDescendant` from
//! `nvdaHelper/remote/IA2Support.cpp:229-312`. Recursive IA2 hypertext
//! walk that locates a content descendant for caret / selection / first
//! / last navigation.

use crate::interfaces::{IAccessible2, IAccessibleHypertext, IAccessibleText};
use windows::core::{Interface, VARIANT};
use windows::Win32::UI::Accessibility::IAccessible;

/// Discriminant for the `what` parameter. Pre-filtered by the C++ caller
/// to one of these five values; out-of-range tags yield `false` from the
/// shim.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindWhat {
    First = 0,
    Caret = 1,
    Last = 2,
    SelectionStart = 3,
    SelectionEnd = 4,
}

impl FindWhat {
    fn from_raw(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::First),
            1 => Some(Self::Caret),
            2 => Some(Self::Last),
            3 => Some(Self::SelectionStart),
            4 => Some(Self::SelectionEnd),
            _ => None,
        }
    }

    /// `Last` and `SelectionEnd` iterate children in reverse order.
    fn is_reverse(&self) -> bool {
        matches!(self, Self::Last | Self::SelectionEnd)
    }
}

/// C-callable replacement for `findContentDescendant`.
///
/// On `true`, both `descendant_id` and `descendant_offset` are written.
/// On `false`, neither is written -- the C++ caller is expected to read
/// them only on `true`, matching the original contract.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `descendant_id` and `descendant_offset` must be valid writable
///   `int*` pointers on success; null is rejected up front.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_find_content_descendant(
    pacc2: *mut core::ffi::c_void,
    what: u32,
    descendant_id: *mut i32,
    descendant_offset: *mut i32,
) -> bool {
    if pacc2.is_null() || descendant_id.is_null() || descendant_offset.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let what = match FindWhat::from_raw(what) {
        Some(w) => w,
        None => return false,
    };
    match unsafe { find_content_descendant(acc2, what) } {
        Some((id, off)) => {
            unsafe {
                *descendant_id = id;
                *descendant_offset = off;
            }
            true
        }
        None => false,
    }
}

/// Pure-Rust port of `findContentDescendant`. Returns `Some((id, offset))`
/// when a content descendant is found, `None` otherwise.
unsafe fn find_content_descendant(
    pacc2: &IAccessible2,
    what: FindWhat,
) -> Option<(i32, i32)> {
    // If this node is text-bearing, work the offset path.
    if let Ok(text) = pacc2.cast::<IAccessibleText>() {
        let offset: i32 = match what {
            FindWhat::First => 0,
            FindWhat::Caret => unsafe { text.get_caretOffset() }.ok()?,
            FindWhat::Last => {
                let n = unsafe { text.get_nCharacters() }.unwrap_or(0);
                if n > 0 { n - 1 } else { 0 }
            }
            FindWhat::SelectionStart | FindWhat::SelectionEnd => {
                let n = unsafe { text.get_nSelections() }.unwrap_or(0);
                if n == 0 {
                    return None;
                }
                let (start, end) = unsafe { text.get_selection(0) }.ok()?;
                if matches!(what, FindWhat::SelectionStart) { start } else { end - 1 }
            }
        };

        // If this offset lands on an embedded hyperlink, recurse into the
        // hyperlinked child.
        if let Ok(hyper) = pacc2.cast::<IAccessibleHypertext>() {
            let hi = unsafe { hyper.get_hyperlinkIndex(offset) }.unwrap_or(-1);
            if hi >= 0 {
                if let Ok(hyperlink) = unsafe { hyper.get_hyperlink(hi) } {
                    if let Ok(child) = hyperlink.cast::<IAccessible2>() {
                        if let Some(found) =
                            unsafe { find_content_descendant(&child, what) }
                        {
                            return Some(found);
                        }
                        // Caret fallback: if Caret didn't resolve in the
                        // child, try First inside the same child. Mirrors
                        // C++ lines 280-282.
                        if matches!(what, FindWhat::Caret) {
                            if let Some(found) = unsafe {
                                find_content_descendant(&child, FindWhat::First)
                            } {
                                return Some(found);
                            }
                        }
                    }
                }
            }
        }

        // No deeper descendant; this node is the answer.
        let id = unsafe { pacc2.get_uniqueID() }.ok()?;
        return Some((id, offset));
    }

    // Not text-bearing; iterate children. LAST / SELECTIONEND iterate
    // in reverse order.
    let pacc: &IAccessible = pacc2;
    let child_count = unsafe { pacc.accChildCount() }.unwrap_or(0);
    if child_count <= 0 {
        return None;
    }
    for i in 1..=child_count {
        let idx = if what.is_reverse() {
            child_count - (i - 1)
        } else {
            i
        };
        let varchild = VARIANT::from(idx);
        let child_disp = match unsafe { pacc.get_accChild(&varchild) } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let child_acc2: IAccessible2 = match child_disp.cast() {
            Ok(a) => a,
            Err(_) => continue,
        };
        if let Some(found) = unsafe { find_content_descendant(&child_acc2, what) } {
            return Some(found);
        }
    }
    None
}
```

* [ ] **Step 2: Add the module to the crate root**

In `rust/nvda_ia2/src/lib.rs`, find the existing module declarations:

```rust
pub mod attribs;
pub mod fetch;
pub mod interfaces;
pub mod live_regions;
pub mod text;
pub mod types;
```

Insert `pub mod find_descendant;` in alphabetical position (between `fetch` and `interfaces`):

```rust
pub mod attribs;
pub mod fetch;
pub mod find_descendant;
pub mod interfaces;
pub mod live_regions;
pub mod text;
pub mod types;
```

* [ ] **Step 3: Verify build, tests, clippy**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
```

Expected: clean build, no warnings, 47 tests still pass (no new tests).

* [ ] **Step 4: Commit**

```sh
git add rust/nvda_ia2/src/find_descendant.rs rust/nvda_ia2/src/lib.rs
git commit -m "Port findContentDescendant to Rust with extern C shim"
```

---

## Task 3: Wire C++ delegation in `IA2Support.cpp`

**Files:**

* Modify: `nvdaHelper/remote/IA2Support.cpp`

The four `FINDCONTENTDESCENDANT_*` constants stay in C++ (they're consumed by the RPC entry and other call sites). Only the body of `findContentDescendant` becomes a thin shim on x86_64; the original implementation is preserved verbatim under `#else`.

* [ ] **Step 1: Replace the body of `findContentDescendant`**

Find the existing definition (starts at line 229 with `bool findContentDescendant(IAccessible2* pacc2, ...`). Replace ONLY that function (do not touch the surrounding constants, `getIA2`, `nvdaInProcUtils_*`, hooks, etc.) with the following `#ifdef _M_X64` / `#else` block:

```cpp
#ifdef _M_X64
extern "C" {
	bool nvda_ia2_find_content_descendant(
		void* pacc2,
		unsigned int what,
		int* descendant_id,
		int* descendant_offset);
}

bool findContentDescendant(IAccessible2* pacc2, long what, long* descendantID, long* descendantOffset) {
	int id = 0;
	int off = 0;
	bool ok = nvda_ia2_find_content_descendant(
		pacc2,
		static_cast<unsigned int>(what),
		&id,
		&off);
	if (ok) {
		*descendantID = id;
		*descendantOffset = off;
	}
	return ok;
}
#else
bool findContentDescendant(IAccessible2* pacc2, long what, long* descendantID, long* descendantOffset) {
	bool foundDescendant=false;
	IAccessibleText* paccText=NULL;
	pacc2->QueryInterface(IID_IAccessibleText,(void**)&paccText);
	if(paccText) {
		long offset=-1;
		switch(what) {
			case FINDCONTENTDESCENDANT_FIRST:
				offset=0;
				break;
			case FINDCONTENTDESCENDANT_CARET:
				paccText->get_caretOffset(&offset);
				break;
			case FINDCONTENTDESCENDANT_LAST:
				paccText->get_nCharacters(&offset);
				// If there is no text, last is still valid but should just use 0.
				if (offset > 0)
					--offset;
				break;
			case FINDCONTENTDESCENDANT_SELECTIONSTART:
			case FINDCONTENTDESCENDANT_SELECTIONEND:
				long nSelections=0;
				paccText->get_nSelections(&nSelections);
				if(nSelections==0) {
					offset=-1;
				} else {
					long startOffset=0;
					long endOffset=0;
					paccText->get_selection(0,&startOffset,&endOffset);
					offset=(what==FINDCONTENTDESCENDANT_SELECTIONSTART)?startOffset:endOffset-1;
				}
				break;
		}
		paccText->Release();
		if(offset==-1) return false;
		IAccessibleHypertext* paccHypertext=NULL;
		pacc2->QueryInterface(IID_IAccessibleHypertext,(void**)&paccHypertext);
		if(paccHypertext) {
			long hi=-1;
			paccHypertext->get_hyperlinkIndex(offset,&hi);
			IAccessibleHyperlink* paccHyperlink=NULL;
			if(hi>=0) {
				paccHypertext->get_hyperlink(hi,&paccHyperlink);
			}
			paccHypertext->Release();
			if(paccHyperlink) {
				IAccessible2* pacc2Child=NULL;
				paccHyperlink->QueryInterface(IID_IAccessible2,(void**)&pacc2Child);
				paccHyperlink->Release();
				if(pacc2Child) {
					foundDescendant=findContentDescendant(pacc2Child,what,descendantID,descendantOffset);
					if(!foundDescendant&&what==FINDCONTENTDESCENDANT_CARET) {
						foundDescendant=findContentDescendant(pacc2Child,FINDCONTENTDESCENDANT_FIRST,descendantID,descendantOffset);
					}
					pacc2Child->Release();
				}
			}
		}
		if(!foundDescendant) {
			pacc2->get_uniqueID(descendantID);
			*descendantOffset=offset;
			foundDescendant=true;
		}
	} else {
		long childCount=0;
		pacc2->get_accChildCount(&childCount);
		VARIANT varChild;
		varChild.vt=VT_I4;
		for(int i=1;i<=childCount;++i) {
			varChild.lVal=(what==FINDCONTENTDESCENDANT_LAST||what==FINDCONTENTDESCENDANT_SELECTIONEND)?(childCount-(i-1)):i;
			IDispatch* pdispatchChild=NULL;
			pacc2->get_accChild(varChild,&pdispatchChild);
			if(!pdispatchChild) continue;
			IAccessible2* pacc2Child=NULL;
			pdispatchChild->QueryInterface(IID_IAccessible2,(void**)&pacc2Child);
			pdispatchChild->Release();
			if(!pacc2Child) continue;
			foundDescendant=findContentDescendant(pacc2Child,what,descendantID,descendantOffset);
			pacc2Child->Release();
			if(foundDescendant) break;
		}
	}
	return foundDescendant;
}
#endif
```

The `#else` body MUST be byte-for-byte identical to the pre-PR original. Don't trim comments, don't reflow whitespace.

* [ ] **Step 2: Build the helper DLL**

```sh
scons.bat source\lib\x64\nvdaHelperRemote.dll
```

(Long timeout, ~600000 ms.) Expected: clean build, no warnings (`/WX` is on). All previous PR's link-line additions (`propsys`, the windows-rs feature gates) carry forward.

* [ ] **Step 3: Commit**

```sh
git add nvdaHelper/remote/IA2Support.cpp
git commit -m "Delegate findContentDescendant to Rust on x86_64"
```

---

## Task 4 (manual): Smoke-test in Firefox

* Run `runnvda.bat` to launch the dev build.
* Open Firefox on a structured page (any article with headings + links works).
* Use `Ctrl+Home` and `Ctrl+End` to confirm caret navigation lands at the start / end of content (FIRST / LAST).
* Use `H` browse-mode key to land on a heading; arrow into a link; observe NVDA reads the link target.
* Select text with shift-arrow; confirm NVDA's selection announcements are correct (SELECTIONSTART / SELECTIONEND).
* Open NVDA's log viewer (`NVDA+F1`) and confirm there are no `panic` / `nvda_ia2` error entries.
* If any regression is observed, do not push -- investigate first.

## Task 5 (manual): Push

```sh
git push origin worktree-rust-beep-generator
```
