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
    let bstr = match acc.get_attributes() {
        Ok(b) => b,
        Err(_) => return false,
    };
    // Mirror C++ ia2utils.cpp:25 (`if (!attribs) return false`): only a
    // NULL BSTR signals "no attributes", a non-NULL zero-length BSTR is
    // treated as a successful (but empty) attributes string and parsed
    // normally (yielding zero callback invocations). `BSTR::is_empty()`
    // returns true for both NULL and zero-length, so it can't be used here.
    //
    // `windows-strings 0.1.0`'s `BSTR` is `#[repr(transparent)]` over a
    // single `*const u16` field (private, no public accessor). The cleanest
    // way to inspect the raw pointer without consuming the BSTR is to
    // reinterpret a `&BSTR` as a `&*const u16` via the transparent layout.
    // SAFETY: `BSTR` is documented `#[repr(transparent)] (*const u16)` in
    // windows-strings; reading the pointer through a shared reference is a
    // valid use of repr(transparent).
    let raw_ptr: *const u16 = unsafe { *(&bstr as *const _ as *const *const u16) };
    if raw_ptr.is_null() {
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
