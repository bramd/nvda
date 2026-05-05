# Rust port of `HyperlinkGetter` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the `HyperlinkGetter` family from `nvdaHelper/common/ia2utils.cpp` to Rust on x86_64, exposed to C++ via an opaque-pointer iterator FFI. Non-x86_64 keeps the verbatim C++ under `#ifdef _M_X64`.

**Architecture:** A new `hyperlink_getter` module in the `nvda_ia2` crate owns a Rust enum (`Ht` / `Ht2` variants) with a `next()` method. Three `extern "C"` shims (`make` / `next` / `free`) expose a `Box<HyperlinkGetter>` opaque handle to C++. A new private `RustHyperlinkGetter` subclass in `ia2utils.cpp` is the only x86_64 `HyperlinkGetter` impl; `Ht/Ht2HyperlinkGetter` move from the public header into the `#else` branch (preserved verbatim).

**Tech Stack:** Rust crate `nvda_ia2`, windows-rs 0.58, scons/MSVC.

Companion design doc: `docs/plans/2026-05-05-rust-hyperlink-getter-design.md`.

---

## Task 1: Add `IAccessibleHypertext2::get_hyperlinks` Rust wrapper

**Files:**

* Modify: `rust/nvda_ia2/src/interfaces.rs`

The vtable slot for `get_hyperlinks` is already typed (PR 1 declared it as a function pointer); only the Rust method wrapper is missing. The IDL signature is `[out, size_is(,*nHyperlinks)] IAccessibleHyperlink ***hyperlinks, [out, retval] long *nHyperlinks`. The output array and its elements are AddRef'd by the server; the client owns both and must `CoTaskMemFree` the outer array and `Release` each element.

* [ ] **Step 1: Add the wrapper method**

In `rust/nvda_ia2/src/interfaces.rs`, find the `IAccessibleHypertext2_Vtbl` struct definition. Immediately after it (before the `// --- IAccessibleHyperlink` comment block), add an `impl IAccessibleHypertext2 { ... }` block:

```rust
impl IAccessibleHypertext2 {
    /// Returns the (server-allocated array, count) of hyperlinks on this
    /// hypertext. Each `Option<IAccessibleHyperlink>` in the array is
    /// AddRef'd; the caller owns them. The caller is also responsible
    /// for freeing the outer array via
    /// `windows::Win32::System::Com::CoTaskMemFree`.
    ///
    /// On error, returns `Err(hr)` and the out-params are not written.
    /// On success with zero links, returns `Ok((null, 0))` -- the caller
    /// should not dereference the array but should still skip the free
    /// (CoTaskMemFree on null is documented as a no-op, so calling it
    /// either way is fine).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext2` implementation for the duration
    /// of this call.
    pub unsafe fn get_hyperlinks(
        &self,
    ) -> windows::core::Result<(*mut Option<IAccessibleHyperlink>, i32)> {
        let mut ptr: *mut Option<IAccessibleHyperlink> = core::ptr::null_mut();
        let mut count: i32 = 0;
        let hr = (Interface::vtable(self).get_hyperlinks)(
            Interface::as_raw(self),
            &mut ptr as *mut _,
            &mut count as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((ptr, count))
    }
}
```

* [ ] **Step 2: Verify build / clippy / tests**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
```

Expected: clean build, all 47 existing tests pass, no clippy warnings.

* [ ] **Step 3: Commit**

```sh
git add rust/nvda_ia2/src/interfaces.rs
git commit -m "Add Rust wrapper for IAccessibleHypertext2::get_hyperlinks"
```

---

## Task 2: Add `hyperlink_getter` Rust module + three `extern "C"` shims

**Files:**

* Create: `rust/nvda_ia2/src/hyperlink_getter.rs`
* Modify: `rust/nvda_ia2/src/lib.rs` (add `pub mod hyperlink_getter;`)

This task is pure-additive Rust -- no C++ changes yet. The three new shims (`make`, `next`, `free`) appear in the static lib but no caller exists, so they're dead code at link time on x86_64 (the linker will keep them since they're `#[no_mangle] extern`).

* [ ] **Step 1: Create the module**

Use the Write tool to create `rust/nvda_ia2/src/hyperlink_getter.rs` with this exact content:

```rust
//! Port of the `HyperlinkGetter` family from
//! `nvdaHelper/common/ia2utils.cpp`.
//!
//! Stateful iterator over hyperlinks in either an `IAccessibleHypertext`
//! (one-at-a-time fetch) or an `IAccessibleHypertext2` (batched fetch
//! cached on first `next()`). Exposed to C++ via three `extern "C"`
//! shims with an opaque `Box<HyperlinkGetter>` handle.

use crate::interfaces::{
    IAccessibleHyperlink, IAccessibleHypertext, IAccessibleHypertext2,
};
use windows::core::Interface;
use windows::Win32::System::Com::CoTaskMemFree;

pub enum HyperlinkGetter {
    /// `IAccessibleHypertext` path: fetch one hyperlink at a time via
    /// `get_hyperlink(index)`. Mirrors `HtHyperlinkGetter`.
    Ht {
        hypertext: IAccessibleHypertext,
        index: u32,
    },
    /// `IAccessibleHypertext2` path: fetch all hyperlinks up front via
    /// `get_hyperlinks` (server-allocated CoTaskMem array of
    /// `IAccessibleHyperlink*`), then index into the cached `Vec` on
    /// each `next()`. Mirrors `Ht2HyperlinkGetter`. Lazily fetched.
    Ht2 {
        hypertext: IAccessibleHypertext2,
        links: Option<Vec<Option<IAccessibleHyperlink>>>,
        index: u32,
    },
}

impl HyperlinkGetter {
    /// Returns the next hyperlink (cloned/AddRef'd for the caller), or
    /// `None` when iteration is exhausted. Increments the internal index.
    ///
    /// # Safety
    ///
    /// The `IAccessibleHypertext` / `IAccessibleHypertext2` interface
    /// stored in `self` must remain valid for the duration of the call.
    pub unsafe fn next(&mut self) -> Option<IAccessibleHyperlink> {
        match self {
            HyperlinkGetter::Ht { hypertext, index } => {
                let i = *index as i32;
                *index += 1;
                // get_hyperlink returns Err on out-of-range; treat as
                // exhausted iterator.
                unsafe { hypertext.get_hyperlink(i) }.ok()
            }
            HyperlinkGetter::Ht2 { hypertext, links, index } => {
                if links.is_none() {
                    *links = Some(unsafe { fetch_ht2_links(hypertext) });
                }
                let cached = links.as_mut().expect("just initialised");
                let i = *index as usize;
                *index += 1;
                if i >= cached.len() {
                    return None;
                }
                // Take the entry out of the Vec slot so it's released
                // exactly once -- either now (handed to the caller) or
                // later if the Drop runs over uniterated entries.
                cached[i].take()
            }
        }
    }
}

/// Fetch the full hyperlinks array from an `IAccessibleHypertext2`,
/// take ownership of every entry, and free the outer CoTaskMem array.
/// Returns an empty `Vec` on COM failure, mirroring the C++ behaviour
/// (`maybeFetch` sets `count = 0` on failure).
unsafe fn fetch_ht2_links(
    hypertext: &IAccessibleHypertext2,
) -> Vec<Option<IAccessibleHyperlink>> {
    let (ptr, count) = match unsafe { hypertext.get_hyperlinks() } {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if ptr.is_null() || count <= 0 {
        // CoTaskMemFree(NULL) is a documented no-op; call unconditionally
        // for symmetry with the success path.
        unsafe { CoTaskMemFree(Some(ptr as *const core::ffi::c_void)) };
        return Vec::new();
    }
    let count = count as usize;
    let mut out: Vec<Option<IAccessibleHyperlink>> = Vec::with_capacity(count);
    for i in 0..count {
        // Each slot was written by the COM server with an AddRef'd
        // interface pointer. core::ptr::read transfers ownership of
        // that reference into the Vec; the slot in the source array is
        // left bitwise-copied but no longer accessed.
        let entry = unsafe { core::ptr::read(ptr.add(i)) };
        out.push(entry);
    }
    unsafe { CoTaskMemFree(Some(ptr as *const core::ffi::c_void)) };
    out
}

// --- C ABI shims ----------------------------------------------------------

/// Construct a HyperlinkGetter for the given IAccessible2, prefer
/// IAccessibleHypertext2 over IAccessibleHypertext. Returns `null` if
/// neither interface is supported (or on null input).
///
/// The returned handle must be freed with
/// [`nvda_ia2_hyperlink_getter_free`].
///
/// # Safety
///
/// `pacc2` must be a valid `IAccessible2*` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_make_hyperlink_getter(
    pacc2: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void {
    if pacc2.is_null() {
        return core::ptr::null_mut();
    }
    let acc2: &crate::interfaces::IAccessible2 =
        match crate::interfaces::IAccessible2::from_raw_borrowed(&pacc2) {
            Some(a) => a,
            None => return core::ptr::null_mut(),
        };
    // Prefer IAccessibleHypertext2; fall back to IAccessibleHypertext.
    if let Ok(ht2) = acc2.cast::<IAccessibleHypertext2>() {
        let getter = Box::new(HyperlinkGetter::Ht2 {
            hypertext: ht2,
            links: None,
            index: 0,
        });
        return Box::into_raw(getter) as *mut core::ffi::c_void;
    }
    if let Ok(ht) = acc2.cast::<IAccessibleHypertext>() {
        let getter = Box::new(HyperlinkGetter::Ht {
            hypertext: ht,
            index: 0,
        });
        return Box::into_raw(getter) as *mut core::ffi::c_void;
    }
    core::ptr::null_mut()
}

/// Get the next hyperlink. Returns an AddRef'd `IAccessibleHyperlink*`
/// (caller `Release`s) or `null` if iteration is exhausted or `handle`
/// is null.
///
/// # Safety
///
/// `handle` must be a valid pointer previously returned by
/// [`nvda_ia2_make_hyperlink_getter`] and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_hyperlink_getter_next(
    handle: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void {
    if handle.is_null() {
        return core::ptr::null_mut();
    }
    let getter: &mut HyperlinkGetter =
        unsafe { &mut *(handle as *mut HyperlinkGetter) };
    match unsafe { getter.next() } {
        Some(link) => {
            // Transfer ownership of the AddRef'd pointer to the caller.
            // `Interface::into_raw` consumes the wrapper without dropping
            // (no Release).
            link.into_raw() as *mut core::ffi::c_void
        }
        None => core::ptr::null_mut(),
    }
}

/// Drop the HyperlinkGetter and release any cached hyperlink references.
/// `null` is accepted and is a no-op.
///
/// # Safety
///
/// `handle` must be either null or a pointer previously returned by
/// [`nvda_ia2_make_hyperlink_getter`] and not yet freed. Must not be
/// used after this call returns.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_hyperlink_getter_free(
    handle: *mut core::ffi::c_void,
) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle as *mut HyperlinkGetter) });
}
```

* [ ] **Step 2: Add the module to the crate root**

In `rust/nvda_ia2/src/lib.rs`, find the existing module declarations:

```rust
pub mod attribs;
pub mod fetch;
pub mod find_descendant;
pub mod hyperlink_getter;
pub mod interfaces;
pub mod live_regions;
pub mod text;
pub mod types;
```

Insert `pub mod hyperlink_getter;` in alphabetical position (between `find_descendant` and `interfaces`):

```rust
pub mod attribs;
pub mod fetch;
pub mod find_descendant;
pub mod hyperlink_getter;
pub mod interfaces;
pub mod live_regions;
pub mod text;
pub mod types;
```

* [ ] **Step 3: Verify build / clippy / tests**

```sh
cargo build --manifest-path rust/nvda_ia2/Cargo.toml
cargo clippy --manifest-path rust/nvda_ia2/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/nvda_ia2/Cargo.toml
```

Expected: clean build, no warnings, 47 tests still pass (no new tests).

If `Interface::into_raw` doesn't compile, check the windows-core 0.58 surface — the right method is `Interface::into_raw(self) -> *mut c_void`. If clippy complains about `*mut c_void` cast pattern, use `as *mut _` form.

* [ ] **Step 4: Verify no BOM and no mangled doc comments**

```sh
head -c 4 rust/nvda_ia2/src/hyperlink_getter.rs | xxd
```

Must show `2f2f 2120` (`//!`). If `efbb bf2f`, rewrite from scratch with the Write tool.

Visually scan the new doc comments for backtick-bound identifiers being stripped (the recurring Unicode-mangling failure mode).

* [ ] **Step 5: Commit**

```sh
git add rust/nvda_ia2/src/hyperlink_getter.rs rust/nvda_ia2/src/lib.rs
git commit -m "Add Rust HyperlinkGetter module with extern C iterator shims"
```

---

## Task 3: Reduce `ia2utils.h` public API and move Ht/Ht2 to .cpp `#else` body

**Files:**

* Modify: `nvdaHelper/common/ia2utils.h`
* Modify: `nvdaHelper/common/ia2utils.cpp`

This is a pure refactor of existing C++ -- no behaviour change on either arch yet. The `Ht/Ht2HyperlinkGetter` classes had public declarations in the header; they had no consumers outside `ia2utils.cpp`. We hide them inside the .cpp `#else` branch and shrink the header to the minimum public surface (`HyperlinkGetter` with pure-virtual `next()` and `makeHyperlinkGetter`).

* [ ] **Step 1: Reshape `ia2utils.h`**

Replace the existing `HyperlinkGetter` / `HtHyperlinkGetter` / `Ht2HyperlinkGetter` class declarations (lines ~45-92 of the current file) with:

```cpp
/**
 * Base class to support retrieving hyperlinks (embedded objects) from
 * IAccessibleHypertext or IAccessibleHypertext2.
 * Construct via the makeHyperlinkGetter factory function below.
 */
class HyperlinkGetter {
	public:
	virtual ~HyperlinkGetter() {}
	/** Get the next hyperlink, or null if iteration is exhausted. */
	virtual CComPtr<IAccessibleHyperlink> next() = 0;
};

/**
 * Create an appropriate HyperlinkGetter to retrieve hyperlinks
 * (embedded objects) if they are supported.
 * IAccessibleHypertext2 will be used in preference to IAccessibleHypertext.
 * @param acc The accessible to use.
 * @return A pointer to the HyperlinkGetter
 *  or a null pointer if hyperlinks aren't supported.
 */
std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc);
```

(Leave the `getAccessibleChildren`, `IA2AttribsToMap`, `fetchIA2Attributes` declarations and the include guards alone.)

* [ ] **Step 2: Move Ht/Ht2 definitions into `ia2utils.cpp`'s `#else` branch**

In `nvdaHelper/common/ia2utils.cpp`, find the existing implementations: `HyperlinkGetter::next()`, the constructors and `get`/`maybeFetch` methods of `HtHyperlinkGetter` / `Ht2HyperlinkGetter`, and `makeHyperlinkGetter`. They currently live at the bottom of the file, OUTSIDE the existing `#ifdef _M_X64` / `#else` / `#endif` block.

Move the entire block (every line touching `HyperlinkGetter`, `HtHyperlinkGetter`, `Ht2HyperlinkGetter`, or `makeHyperlinkGetter`) into the `#else` branch's body. Specifically:

1. The current file has `#endif` somewhere near line 121 ending the existing `#else` branch, followed by `getAccessibleChildren` (still outside), then the HyperlinkGetter family (still outside).
2. Move `getAccessibleChildren` to remain OUTSIDE the conditional (it's a separately-deferred port; touch only the HyperlinkGetter family in this PR).
3. Inside the existing `#else` branch, BEFORE its closing `#endif`, insert the Ht/Ht2 class definitions and their implementations.
4. Since the header no longer declares Ht/Ht2 classes, define them in an anonymous namespace at the top of the `#else` branch:

```cpp
namespace {
	class HtHyperlinkGetter : public HyperlinkGetter {
		public:
		HtHyperlinkGetter(CComPtr<IAccessibleHypertext> hypertext)
			: hypertext(hypertext) {}
		CComPtr<IAccessibleHyperlink> next() override;

		private:
		CComPtr<IAccessibleHypertext> hypertext;
		long index = 0;
	};

	class Ht2HyperlinkGetter : public HyperlinkGetter {
		public:
		Ht2HyperlinkGetter(CComPtr<IAccessibleHypertext2> hypertext)
			: hypertext(hypertext), count(-1) {}
		~Ht2HyperlinkGetter() override {
			if (this->rawLinks) {
				CoTaskMemFree(this->rawLinks);
			}
		}
		CComPtr<IAccessibleHyperlink> next() override;

		private:
		CComPtr<IAccessibleHypertext2> hypertext;
		IAccessibleHyperlink** rawLinks = nullptr;
		long count;
		long index = 0;
		void maybeFetch();
	};

	CComPtr<IAccessibleHyperlink> HtHyperlinkGetter::next() {
		CComPtr<IAccessibleHyperlink> link;
		// hyperlink will fail or return null if the index is too big.
		HRESULT res = this->hypertext->get_hyperlink(this->index, &link);
		++this->index;
		if (FAILED(res) || !link) {
			return nullptr;
		}
		return link;
	}

	void Ht2HyperlinkGetter::maybeFetch() {
		if (this->count >= 0) {
			return;
		}
		if (FAILED(hypertext->get_hyperlinks(&this->rawLinks, &this->count))) {
			this->count = 0;
		}
	}

	CComPtr<IAccessibleHyperlink> Ht2HyperlinkGetter::next() {
		this->maybeFetch();
		if (this->index >= this->count) {
			return nullptr;
		}
		// Ensure we don't AddRef this pointer.
		CComPtr<IAccessibleHyperlink> link;
		link.Attach(this->rawLinks[this->index]);
		++this->index;
		return link;
	}
}

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc) {
	// Try IAccessibleHypertext2 first.
	CComQIPtr<IAccessibleHypertext2> ht2 = acc;
	if (ht2) {
		return std::make_unique<Ht2HyperlinkGetter>(ht2);
	}
	// Fall back to IAccessibleHypertext.
	CComQIPtr<IAccessibleHypertext> ht = acc;
	if (ht) {
		return std::make_unique<HtHyperlinkGetter>(ht);
	}
	// Neither interface is supported.
	return nullptr;
}
```

(Note the small refactor: `next()` is now overridden directly on Ht / Ht2 instead of the previous pattern of overriding `get(index)` and inheriting a base-class `next()` that called it. This is needed because the new header declares `next()` as pure virtual on `HyperlinkGetter` -- there's no inherited default. The visible behaviour is identical.)

Delete the OLD HyperlinkGetter / HtHyperlinkGetter / Ht2HyperlinkGetter / makeHyperlinkGetter implementations that previously lived OUTSIDE the conditional (the ones currently at the bottom of the file).

* [ ] **Step 3: Build the helper DLL on x86_64**

```sh
scons.bat source\lib\x64\nvdaHelperRemote.dll
```

(Long timeout, ~600000 ms.) Expected: clean build. The x86_64 path now has no `HyperlinkGetter` implementation -- `gecko_ia2.cpp` calls `makeHyperlinkGetter` which is no longer defined for x86_64.

This will fail to link with an unresolved symbol for `makeHyperlinkGetter`. That's expected and intentional -- Task 4 wires the x86_64 implementation. Confirm the link error is specifically `makeHyperlinkGetter` unresolved and not something unrelated.

* [ ] **Step 4: Commit**

```sh
git add nvdaHelper/common/ia2utils.h nvdaHelper/common/ia2utils.cpp
git commit -m "Reduce HyperlinkGetter public API and move Ht/Ht2 to non-x86_64 fallback"
```

(The x86_64 build will be temporarily broken between this commit and the next. The non-x86_64 build is unaffected.)

---

## Task 4: Wire `ia2utils.cpp` x86_64 branch to use the Rust shims

**Files:**

* Modify: `nvdaHelper/common/ia2utils.cpp`

* [ ] **Step 1: Add the Rust extern declarations + RustHyperlinkGetter to the existing `#ifdef _M_X64` branch**

Find the existing `#ifdef _M_X64` block at the top of `ia2utils.cpp` -- it currently contains the `nvda_ia2_attribs_to_map` and `nvda_ia2_fetch_attributes` extern declarations and the `insert_into_map` callback. Append the following inside that same `extern "C"` block (alongside the existing prototypes):

```c
	void* nvda_ia2_make_hyperlink_getter(void* pacc2);
	void* nvda_ia2_hyperlink_getter_next(void* handle);
	void  nvda_ia2_hyperlink_getter_free(void* handle);
```

Then, AFTER the `insert_into_map` namespace block but still inside the `#ifdef _M_X64` branch (before the existing `bool fetchIA2Attributes(...)` and `void IA2AttribsToMap(...)` definitions), add:

```cpp
namespace {
	class RustHyperlinkGetter : public HyperlinkGetter {
		public:
		explicit RustHyperlinkGetter(void* h) : handle(h) {}
		~RustHyperlinkGetter() override {
			if (handle) {
				nvda_ia2_hyperlink_getter_free(handle);
			}
		}
		// No copy, no assign.
		RustHyperlinkGetter(const RustHyperlinkGetter&) = delete;
		RustHyperlinkGetter& operator=(const RustHyperlinkGetter&) = delete;

		CComPtr<IAccessibleHyperlink> next() override {
			CComPtr<IAccessibleHyperlink> link;
			if (!handle) {
				return link;
			}
			auto* raw = static_cast<IAccessibleHyperlink*>(
				nvda_ia2_hyperlink_getter_next(handle));
			// raw is already AddRef'd by the Rust side or null;
			// Attach takes ownership without extra AddRef.
			link.Attach(raw);
			return link;
		}

		private:
		void* handle;
	};
}

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc) {
	void* handle = nvda_ia2_make_hyperlink_getter(acc);
	if (!handle) {
		return nullptr;
	}
	return std::make_unique<RustHyperlinkGetter>(handle);
}
```

* [ ] **Step 2: Build the Rust workspace lib for SCons**

(Per the prior-PR memo: SCons looks at `build/rust/release/nvda_ia2.lib`; running plain `cargo build` writes to `rust/target/release/` instead. Force the SCons-managed cargo build to refresh the lib.)

```sh
cargo build --release --target-dir build/rust --manifest-path rust/Cargo.toml --package nvda_ia2 --package nvda_input_hooks
```

Expected: clean build.

* [ ] **Step 3: Build the helper DLL on x86_64**

```sh
scons.bat source\lib\x64\nvdaHelperRemote.dll
```

(Long timeout, ~600000 ms.) Expected: clean build, no warnings (`/WX` is on). All previous PRs' link-line additions (`propsys`, the windows-rs feature gates) carry forward.

If the link fails on additional unresolved windows-rs symbols, do NOT modify SCons -- stop and report.

* [ ] **Step 4: Commit**

```sh
git add nvdaHelper/common/ia2utils.cpp
git commit -m "Delegate HyperlinkGetter to Rust on x86_64"
```

---

## Task 5 (manual): Smoke-test in Firefox

After the agent reports Tasks 1-4 complete, the human operator verifies in Firefox:

* Run `runnvda.bat` to launch the dev build.
* Open Firefox on a structured page with multiple links inside paragraphs (any article works -- Wikipedia is a good test bed).
* Use browse-mode line/word reading (`down arrow`, arrow into links) and confirm link text is announced correctly. The hyperlink iteration is what `HyperlinkGetter::next()` drives during vbuf rendering for embedded-object characters.
* Use `K` browse-mode key to jump between links. Confirm next/previous link nav works.
* Open NVDA's log viewer (`NVDA+F1`) and confirm there are no `panic` / `nvda_ia2` error entries.
* If any regression is observed, do not push -- investigate first.

## Task 6 (manual): Push

Once smoke-test passes:

```sh
git push origin worktree-rust-beep-generator
```
