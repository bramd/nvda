//! Port of `getAccDescription` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:324`.
//!
//! Wraps `IAccessible::get_accDescription`. Returns `true` and invokes
//! `cb` once with the description content when the COM call succeeds
//! and the BSTR is non-null. Returns `false` (callback never invoked)
//! when the COM call fails. A successful call with an empty (zero-
//! length non-null) BSTR still invokes `cb` with `len == 0`.

use crate::interfaces::IAccessible2;
use windows::core::{Interface, VARIANT};
use windows::Win32::UI::Accessibility::IAccessible;

/// Callback invoked at most once with the description content. The
/// pointer + length describe the wide-string range (excluding any null
/// terminator); the callback must copy the data before returning.
pub type AccDescriptionCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    description_ptr: *const u16,
    description_len: usize,
);

/// `BSTR::is_empty()` returns true for both NULL and zero-length BSTRs.
/// We need to distinguish: NULL means "no description" (mirroring how
/// the C++ original would UB on a null `m_str`), zero-length means
/// "empty string description" which the original would represent as
/// `Some("")`. SAFETY: `windows::core::BSTR` is `repr(transparent)`
/// over a single `*const u16`; same trick used in `fetch.rs`.
fn is_bstr_null(bstr: &windows::core::BSTR) -> bool {
    let raw_ptr: *const u16 =
        unsafe { *(bstr as *const _ as *const *const u16) };
    raw_ptr.is_null()
}

/// Rust-native variant of `getAccDescription` for in-crate callers.
/// Returns `Some(wide-chars)` when the COM call succeeded and produced
/// a non-NULL BSTR (the empty string is preserved as `Some(vec![])`).
/// Returns `None` on failure or NULL BSTR.
pub(crate) fn get_acc_description_native(
    acc: &IAccessible2,
    childid: i32,
) -> Option<Vec<u16>> {
    let pacc_msaa: &IAccessible = acc;
    let varchild = VARIANT::from(childid);
    let desc = match unsafe { pacc_msaa.get_accDescription(&varchild) } {
        Ok(d) => d,
        Err(_) => return None,
    };
    if is_bstr_null(&desc) {
        return None;
    }
    Some(desc.as_wide().to_vec())
}

/// C-callable replacement for `getAccDescription`. Returns `true` when
/// a description was retrieved (callback invoked once), `false`
/// otherwise.
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*` for the duration of the call.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind across the FFI boundary.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_acc_description(
    pacc: *mut core::ffi::c_void,
    childid: i32,
    ctx: *mut core::ffi::c_void,
    cb: AccDescriptionCallback,
) -> bool {
    if pacc.is_null() {
        return false;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return false,
    };
    let pacc_msaa: &IAccessible = acc;
    let varchild = VARIANT::from(childid);
    let desc = match unsafe { pacc_msaa.get_accDescription(&varchild) } {
        Ok(d) => d,
        Err(_) => return false,
    };
    if is_bstr_null(&desc) {
        return false;
    }
    let slice = desc.as_wide();
    unsafe { cb(ctx, slice.as_ptr(), slice.len()) };
    true
}
