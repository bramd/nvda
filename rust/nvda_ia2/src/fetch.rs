//! Port of `fetchIA2Attributes` from `nvdaHelper/common/ia2utils.cpp:22`.
//!
//! Calls `IAccessible2::get_attributes`, hands the resulting BSTR to the
//! attributes parser, and invokes a per-pair callback so the C++ wrapper
//! can populate its `std::map<std::wstring, std::wstring>&`.

use std::collections::BTreeMap;

use crate::attribs::{parse_attribs, AttribCallback};
use crate::bstr::is_bstr_null;
use crate::interfaces::IAccessible2;
use windows::core::Interface;

/// Rust-native variant of `fetchIA2Attributes` for in-crate callers.
/// Returns `Some(map)` when the call succeeds and returns a non-NULL
/// BSTR (an empty attribute string parses to an empty map, still
/// `Some`). Returns `None` when the COM call fails or returns a NULL
/// BSTR (the C++ "no attributes" sentinel). Keeping the NULL-vs-empty
/// distinction here lets the FFI shim report it faithfully.
pub(crate) fn fetch_ia2_attributes_native(
    acc: &IAccessible2,
) -> Option<BTreeMap<String, String>> {
    let bstr = match unsafe { acc.get_attributes() } {
        Ok(b) => b,
        Err(_) => return None,
    };
    // NULL BSTR == no attributes; an empty (non-NULL) BSTR is a
    // successful empty parse.
    if is_bstr_null(&bstr) {
        return None;
    }
    let s = bstr.to_string();
    Some(parse_attribs(&s))
}

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
/// - `cb` must not unwind. C++ exceptions thrown out of the callback would
///   propagate through the `extern "C"` frame, which is undefined behavior
///   on stable Rust. In the planned C++ adapter (`ia2utils.cpp`), the only
///   realistic throw is `std::bad_alloc` from `std::map::emplace` or
///   `std::wstring` construction; the adapter must catch (or accept process
///   termination on OOM) before returning to Rust.
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
    // The native helper preserves the "COM call returned NULL BSTR"
    // (`None`, reported as `false`) versus "valid empty attribute
    // string" (`Some(empty)`, reported as `true` with zero callbacks)
    // distinction, so we can delegate to it directly.
    let map = match fetch_ia2_attributes_native(acc) {
        Some(m) => m,
        None => return false,
    };
    for (k, v) in map {
        let k_utf16: Vec<u16> = k.encode_utf16().collect();
        let v_utf16: Vec<u16> = v.encode_utf16().collect();
        unsafe {
            cb(
                ctx,
                k_utf16.as_ptr(),
                k_utf16.len(),
                v_utf16.as_ptr(),
                v_utf16.len(),
            );
        }
    }
    true
}
