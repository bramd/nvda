//! Port of `getTextBoxInComboBox` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:392`.
//!
//! Returns the first child of a combo box if it is a focusable text
//! field, otherwise `None`. Used by gecko's vbuf rendering to find
//! editable text inside a combo box.

use crate::interfaces::IAccessible2;
use windows::core::{Interface, VARIANT};
use windows::Win32::UI::Accessibility::{IAccessible, ROLE_SYSTEM_TEXT};
use windows::Win32::UI::Controls::STATE_SYSTEM_FOCUSABLE;

/// VT_I4 per OAIDL.h.
const VT_I4_RAW: u16 = 3;
/// CHILDID_SELF per oleacc.h.
const CHILDID_SELF: i32 = 0;

/// If the first child of `combo_box` is a focusable text field, return
/// it. Otherwise `None`.
///
/// # Safety
///
/// `combo_box` must be a valid `IAccessible2` for the duration of the
/// call.
pub unsafe fn get_text_box_in_combo_box(
    combo_box: &IAccessible2,
) -> Option<IAccessible2> {
    let pacc: &IAccessible = combo_box;
    // First child only; `accChild(1)` per the IAccessible IDL.
    let child_disp = unsafe { pacc.get_accChild(&VARIANT::from(1i32)) }.ok()?;
    let child: IAccessible2 = child_disp.cast().ok()?;

    // role() returns the IA2 role; if call fails, bail.
    let role = unsafe { child.role() }.ok()?;
    if role != ROLE_SYSTEM_TEXT as i32 {
        return None;
    }

    // Check focusable bit on accState.
    let pacc_child: &IAccessible = &child;
    let state =
        unsafe { pacc_child.get_accState(&VARIANT::from(CHILDID_SELF)) }.ok()?;
    let raw = state.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt != VT_I4_RAW {
        return None;
    }
    let lval = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
    if (lval & (STATE_SYSTEM_FOCUSABLE.0 as i32)) == 0 {
        return None;
    }
    Some(child)
}

/// C-callable shim. Returns an AddRef'd `IAccessible2*` (caller `Release`s)
/// or `null`.
///
/// # Safety
///
/// `combo_box` must be a valid `IAccessible2*` for the duration of the
/// call.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_text_box_in_combo_box(
    combo_box: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void {
    if combo_box.is_null() {
        return core::ptr::null_mut();
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&combo_box) {
        Some(a) => a,
        None => return core::ptr::null_mut(),
    };
    match unsafe { get_text_box_in_combo_box(acc) } {
        Some(t) => t.into_raw(),
        None => core::ptr::null_mut(),
    }
}
