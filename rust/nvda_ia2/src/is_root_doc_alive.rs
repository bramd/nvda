//! Port of `GeckoVBufBackend_t::isRootDocAlive` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:1361` (pre-flip).
//!
//! Decides whether the document this backend is rendering for is
//! still alive. The C++ original short-circuits on a pending update
//! (don't bother with a COM call when we already know the document
//! is in flux), then checks the cached root `IAccessible2` for a
//! `IA2_STATE_DEFUNCT` flag. If the document is dead the caller
//! clears its cached root accessible.

use core::ffi::c_void;

use windows::core::Interface;

use crate::interfaces::IAccessible2;
use nvda_vbuf::VbufBackend;

/// `IA2_STATE_DEFUNCT` from `AccessibleStates.idl:93`.
const IA2_STATE_DEFUNCT: i32 = 0x4;

/// C-callable replacement for `isRootDocAlive`.
///
/// Returns:
/// * non-zero — the root document is alive; the caller's cached
///   `rootDocAcc` should be left in place.
/// * zero — the root document is dead; the caller should clear its
///   cached `rootDocAcc` (`CComPtr::operator=(nullptr)` or equivalent).
///
/// # Safety
///
/// * `backend` must be a valid `VBufBackend_t*` for the duration.
/// * `root_doc_acc` may be NULL (treated as dead) or a valid
///   borrowed `IAccessible2*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_is_root_doc_alive(
    backend: *mut c_void,
    root_doc_acc: *mut c_void,
) -> i32 {
    if backend.is_null() {
        return 0;
    }
    let backend = VbufBackend(backend);

    // Fast-path: if there's a pending update we don't bother with the
    // COM call -- mirrors the C++ early return at gecko_ia2.cpp:1362.
    let pending_empty =
        unsafe { backend.pending_invalid_subtrees_empty() };
    if !pending_empty {
        return 1;
    }

    if root_doc_acc.is_null() {
        return 0;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&root_doc_acc) {
        Some(a) => a,
        None => return 0,
    };
    let states = match unsafe { acc.get_states() } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    if (states & IA2_STATE_DEFUNCT) != 0 {
        return 0;
    }
    1
}
