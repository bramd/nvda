//! Port of `GeckoVBufBackend_t::versionSpecificInit` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:197`.
//!
//! Walks IAccessible2 → IServiceProvider → IAccessibleApplication →
//! `get_toolkitName` and returns the resulting wide string to the
//! caller via callback. The C++ side stores the result in
//! `this->toolkitName` for the existing `is_chrome` check used
//! throughout the file.

use core::ffi::c_void;

use windows::core::Interface;
use windows::Win32::System::Com::IServiceProvider;

use crate::interfaces::{IAccessible2, IAccessibleApplication};

/// Callback receiving the toolkit name as a (ptr, len) UTF-16 slice
/// borrowed for the duration of the call.
pub type ToolkitNameCallback =
    unsafe extern "C" fn(ctx: *mut c_void, ptr: *const u16, len: usize);

/// C-callable replacement for `versionSpecificInit`.
///
/// On success the callback is invoked exactly once with the toolkit
/// name and `true` is returned. On any COM failure the callback is
/// not invoked and `false` is returned.
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*` for the duration.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind across the FFI boundary.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_toolkit_name(
    pacc: *mut c_void,
    ctx: *mut c_void,
    cb: ToolkitNameCallback,
) -> bool {
    if pacc.is_null() {
        return false;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return false,
    };
    let serv: IServiceProvider = match acc.cast() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let app: IAccessibleApplication = match unsafe {
        serv.QueryService(&IAccessibleApplication::IID)
    } {
        Ok(a) => a,
        Err(_) => return false,
    };
    let name = match unsafe { app.get_toolkitName() } {
        Ok(n) => n,
        Err(_) => return false,
    };
    // BSTR is `*const u16`-shaped; `as_wide()` borrows the wide chars.
    let wide = name.as_wide();
    unsafe { cb(ctx, wide.as_ptr(), wide.len()) };
    true
}
