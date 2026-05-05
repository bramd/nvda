//! Port of `GeckoVBufBackend_t::getSelectedItem` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:348-387`.
//!
//! Given an `IAccessible2` container, returns the first selected item, or
//! the first child if no item is selected. Used by the gecko vbuf backend
//! when rendering selection-only collections (combo boxes, listboxes,
//! tab panels).

use crate::interfaces::IAccessible2;
use windows::core::{IUnknown, Interface, VARIANT};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::UI::Accessibility::IAccessible;

/// VT_DISPATCH per OAIDL.h.
const VT_DISPATCH_RAW: u16 = 9;
/// VT_UNKNOWN per OAIDL.h.
const VT_UNKNOWN_RAW: u16 = 13;

/// Try to extract the selected item, falling back to the first child if
/// none is selected.
///
/// `IAccessible::accSelection` returns one of:
/// * a VT_DISPATCH VARIANT — a single selected item, returned directly;
/// * a VT_UNKNOWN VARIANT — an `IEnumVARIANT` over multiple selected
///   items; we return the first;
/// * any other VT (typically VT_EMPTY when nothing is selected) — we fall
///   back to `accChild(1)`.
///
/// Returns `None` if no selectable item can be found.
///
/// # Safety
///
/// `pacc2` must be a live, well-formed `IAccessible2` for the duration
/// of the call.
pub unsafe fn get_selected_item(pacc2: &IAccessible2) -> Option<IAccessible2> {
    let pacc: &IAccessible = pacc2;
    let selection = match unsafe { pacc.accSelection() } {
        Ok(v) => v,
        Err(_) => return None,
    };
    let raw = selection.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };

    if vt == VT_DISPATCH_RAW {
        let disp = IDispatch::try_from(&selection).ok()?;
        return disp.cast::<IAccessible2>().ok();
    }

    if vt == VT_UNKNOWN_RAW {
        // The unknown is an IEnumVARIANT over the selected items; we only
        // care about the first.
        let punk_val = unsafe { raw.Anonymous.Anonymous.Anonymous.punkVal };
        let unk: &IUnknown = unsafe { IUnknown::from_raw_borrowed(&punk_val) }?;
        let enum_var: IEnumVARIANT = unk.cast().ok()?;
        let mut items = [VARIANT::default()];
        let hr =
            unsafe { enum_var.Next(&mut items, core::ptr::null_mut()) };
        if hr.is_err() {
            return None;
        }
        let item_raw = items[0].as_raw();
        let item_vt = unsafe { item_raw.Anonymous.Anonymous.vt };
        if item_vt != VT_DISPATCH_RAW {
            return None;
        }
        let disp = IDispatch::try_from(&items[0]).ok()?;
        return disp.cast::<IAccessible2>().ok();
    }

    // No selection: fall back to the first child. CHILDID `1` is the first
    // child per the IAccessible IDL.
    let varchild = VARIANT::from(1i32);
    let child_disp = unsafe { pacc.get_accChild(&varchild) }.ok()?;
    child_disp.cast::<IAccessible2>().ok()
}

/// C-callable shim. Returns an AddRef'd `IAccessible2*` (caller `Release`s)
/// or `null` on null input or no result.
///
/// # Safety
///
/// `pacc2` must be a valid `IAccessible2*` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_selected_item(
    pacc2: *mut core::ffi::c_void,
) -> *mut core::ffi::c_void {
    if pacc2.is_null() {
        return core::ptr::null_mut();
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return core::ptr::null_mut(),
    };
    match unsafe { get_selected_item(acc2) } {
        Some(item) => item.into_raw(),
        None => core::ptr::null_mut(),
    }
}
