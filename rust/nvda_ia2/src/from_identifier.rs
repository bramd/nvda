//! Port of `IAccessible2FromIdentifier` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:124`.
//!
//! Bridges a `(docHandle, ID)` Win32 identifier pair to an
//! `IAccessible2` interface via `AccessibleObjectFromEvent` followed by
//! `IServiceProvider::QueryService<IAccessible>(IID_IAccessible2)`.

use crate::interfaces::IAccessible2;
use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IServiceProvider;
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromEvent, IAccessible,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

/// `CHILDID_SELF` per oleacc.h. The `varchild.lVal` for a non-simple
/// child accessible.
const CHILDID_SELF: i32 = 0;
/// `VT_I4` per OAIDL.h.
const VT_I4_RAW: u16 = 3;

/// Resolve a `(docHandle, id)` pair to an `IAccessible2`. Returns `None`
/// if `AccessibleObjectFromEvent` fails, the resulting accessible is a
/// simple child (cannot implement IAccessible2), or the QI/QueryService
/// chain fails.
///
/// Mirrors `IAccessible2FromIdentifier` in
/// `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:124-148`. The
/// returned `IAccessible2` is owned (one AddRef); dropping it calls
/// `Release`.
///
/// # Safety
///
/// Calls `AccessibleObjectFromEvent` and walks the resulting COM
/// interfaces. The COM apartment must be initialised; `doc_handle` is
/// reinterpreted as an `HWND` per the C++ original's
/// `(HWND)UlongToHandle(docHandle)` cast and is passed to the Win32
/// API without further validation.
pub unsafe fn from_identifier(doc_handle: i32, id: i32) -> Option<IAccessible2> {
    // Sign-extend the i32 docHandle into the pointer-sized HWND, matching
    // the C++ `(HWND)UlongToHandle(docHandle)` cast.
    let hwnd = HWND(doc_handle as isize as *mut core::ffi::c_void);

    let mut pacc: Option<IAccessible> = None;
    let mut varchild = VARIANT::default();
    if unsafe {
        AccessibleObjectFromEvent(hwnd, OBJID_CLIENT.0 as u32, id as u32, &mut pacc, &mut varchild)
    }
    .is_err()
    {
        return None;
    }
    let pacc = pacc?;

    // IAccessible2 cannot be implemented on a simple child. The C++ checks
    // `varChild.lVal != CHILDID_SELF` -- mirror that. We assume vt is
    // VT_I4 (set by the COM server before returning); if the server
    // produced anything else we treat it as "not a simple child" and
    // continue, matching the C++ which would also pass through
    // (varChild.lVal in C reads a union member regardless of vt).
    let raw = varchild.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    let lval = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
    if vt == VT_I4_RAW && lval != CHILDID_SELF {
        return None;
    }

    let pserv: IServiceProvider = pacc.cast().ok()?;
    let pacc2: IAccessible2 =
        unsafe { pserv.QueryService::<IAccessible2>(&IAccessible::IID) }.ok()?;
    Some(pacc2)
}

/// C-callable shim. Returns an AddRef'd `IAccessible2*` (caller `Release`s)
/// or `null`.
///
/// # Safety
///
/// Same caller obligations as [`from_identifier`]: the COM apartment
/// must be initialised, and `doc_handle` is reinterpreted as an HWND.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_from_identifier(
    doc_handle: i32,
    id: i32,
) -> *mut core::ffi::c_void {
    match unsafe { from_identifier(doc_handle, id) } {
        Some(acc) => acc.into_raw(),
        None => core::ptr::null_mut(),
    }
}
