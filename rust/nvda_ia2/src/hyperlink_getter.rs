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
            link.into_raw()
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
