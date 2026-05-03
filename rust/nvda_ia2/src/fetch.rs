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
