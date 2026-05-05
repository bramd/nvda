//! Port of `getRoleLongRoleString` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:366`.
//!
//! Returns the IA2 role for an `IAccessible2` plus, if the role was not
//! resolvable, falls back to MSAA `accRole` which can yield either an
//! `i32` role (which clobbers the returned role) or a `BSTR` role
//! string (forwarded to a callback once, if present).

use crate::interfaces::IAccessible2;
use windows::core::{Interface, VARIANT, BSTR};
use windows::Win32::UI::Accessibility::IAccessible;

/// VT_I4 / VT_BSTR per OAIDL.h.
const VT_I4_RAW: u16 = 3;
const VT_BSTR_RAW: u16 = 8;

/// Callback invoked exactly once with the role string when the MSAA
/// fallback yielded a BSTR. Pointer + length describe the wide-string
/// content (excluding any null terminator); the callback must copy the
/// data before returning. Not invoked when the role was resolvable as
/// an integer.
pub type RoleStringCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    role_string_ptr: *const u16,
    role_string_len: usize,
);

/// Rust-native variant of `getRoleLongRoleString` for in-crate
/// callers. Returns `(role, optional_role_string_utf16)`. The role
/// string is `Some` only when the MSAA fallback returned a `VT_BSTR`.
pub(crate) fn get_role_long_role_string_native(
    acc: &IAccessible2,
    childid: i32,
) -> (i32, Option<Vec<u16>>) {
    let mut role: i32 = unsafe { acc.role() }.unwrap_or(0);
    if role != 0 {
        return (role, None);
    }
    let pacc_msaa: &IAccessible = acc;
    let varchild = VARIANT::from(childid);
    let varrole = match unsafe { pacc_msaa.get_accRole(&varchild) } {
        Ok(v) => v,
        Err(_) => return (role, None),
    };
    let raw = varrole.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_I4_RAW {
        role = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
        (role, None)
    } else if vt == VT_BSTR_RAW {
        let bstr_ptr = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
        if bstr_ptr.is_null() {
            (role, None)
        } else {
            let borrowed: BSTR = unsafe { BSTR::from_raw(bstr_ptr) };
            // VARIANT owns the BSTR; don't drop the borrow.
            let manual = core::mem::ManuallyDrop::new(borrowed);
            let slice = manual.as_wide().to_vec();
            (role, Some(slice))
        }
    } else {
        (role, None)
    }
}

/// C-callable replacement for `getRoleLongRoleString`. Returns the long
/// IA2 role (or `0` / `IA2_ROLE_UNKNOWN` if neither `IAccessible2::role`
/// nor MSAA `accRole` produced one). If the MSAA fallback returned a
/// `VT_BSTR`, `cb` is invoked once with the string.
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*` for the duration of the call.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind across the FFI boundary.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_role_long_role_string(
    pacc: *mut core::ffi::c_void,
    childid: i32,
    ctx: *mut core::ffi::c_void,
    cb: RoleStringCallback,
) -> i32 {
    if pacc.is_null() {
        return 0;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return 0,
    };
    let (role, role_string) = get_role_long_role_string_native(acc, childid);
    if let Some(s) = role_string {
        unsafe { cb(ctx, s.as_ptr(), s.len()) };
    }
    role
}
