//! Port of `GeckoVBufBackend_t::getLabelInfo` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:274`.
//!
//! Resolves an `IA2_RELATION_LABELLED_BY` relation to its first target,
//! checks whether the label element is visible and has accessible text
//! content, and returns the target's IA2 unique ID. Used by the gecko
//! vbuf backend during render to decide whether to surface a label.

use crate::interfaces::{IAccessible2, IAccessible2_2};
use crate::text::get_text_from_iaccessible_collect;
use windows::core::{IUnknown, Interface, BSTR, VARIANT};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Accessibility::IAccessible;
use windows::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;

/// Wide-string spelling of the `IA2_RELATION_LABELLED_BY` constant from
/// `include/ia2/api/AccessibleRelation.idl:119`.
const LABELLED_BY: &[u16] = &[
    b'l' as u16, b'a' as u16, b'b' as u16, b'e' as u16, b'l' as u16,
    b'l' as u16, b'e' as u16, b'd' as u16, b'B' as u16, b'y' as u16,
];

/// Callback invoked once with the resolved label info if a labelling
/// relation was found. `is_visible` is `1` (true) or `0` (false). When
/// `has_id` is non-zero, `id` is the IA2 unique ID of the label
/// element; otherwise `id` is unspecified.
pub type LabelInfoCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    is_visible: bool,
    has_id: bool,
    id: i32,
);

/// C-callable replacement for `GeckoVBufBackend_t::getLabelInfo`.
/// Returns `true` and invokes `cb` once when label info was found;
/// returns `false` (callback never invoked) otherwise.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind across the FFI boundary.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_label_info(
    pacc2: *mut core::ffi::c_void,
    ctx: *mut core::ffi::c_void,
    cb: LabelInfoCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let acc2_2: IAccessible2_2 = match acc.cast() {
        Ok(a) => a,
        Err(_) => return false,
    };

    let relation_bstr = match BSTR::from_wide(LABELLED_BY) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let (raw_targets, count) =
        match unsafe { acc2_2.get_relationTargetsOfType(&relation_bstr, 1) } {
            Ok(v) => v,
            Err(_) => return false,
        };
    let count = count.max(0) as usize;
    if raw_targets.is_null() || count == 0 {
        unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
        return false;
    }

    // Take ownership of the first entry; release any extras (defensive --
    // we asked for max 1, but a misbehaving server could return more).
    let first_raw = unsafe { core::ptr::read(raw_targets) };
    for i in 1..count {
        let extra = unsafe { core::ptr::read(raw_targets.add(i)) };
        if !extra.is_null() {
            let unk: IUnknown = unsafe { IUnknown::from_raw(extra) };
            drop(unk);
        }
    }
    unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };

    if first_raw.is_null() {
        return false;
    }
    let first_unk: IUnknown = unsafe { IUnknown::from_raw(first_raw) };
    let first_acc: IAccessible2 = match first_unk.cast() {
        Ok(a) => a,
        Err(_) => return false,
    };

    // Check accState for STATE_SYSTEM_INVISIBLE. Mirrors the C++:
    //   bool isVisible = res == S_OK && !(state.lVal & STATE_SYSTEM_INVISIBLE);
    let pacc_first: &IAccessible = &first_acc;
    let varchild = VARIANT::from(0i32); // CHILDID_SELF
    let mut is_visible = false;
    if let Ok(state) = unsafe { pacc_first.get_accState(&varchild) } {
        let raw = state.as_raw();
        let lval = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
        is_visible = (lval & (STATE_SYSTEM_INVISIBLE.0 as i32)) == 0;
    }

    // If visible, also verify the label element has actual accessible
    // text content. A label that is "visible" but has no accessible text
    // (e.g. only aria-hidden children) cannot be the source of the
    // accessible name.
    if is_visible {
        let mut text_buf: Vec<u16> = Vec::new();
        let got_text = get_text_from_iaccessible_collect(
            &mut text_buf,
            &first_acc,
            false, // use_new_text
            true,  // recurse
            true,  // include_top_level_text
        );
        if !got_text {
            is_visible = false;
        }
    }

    // Fetch the IA2 unique ID. C++'s getIAccessible2UniqueID returns
    // Optional<int>: present iff get_uniqueID succeeded.
    let (has_id, id) = match unsafe { first_acc.get_uniqueID() } {
        Ok(v) => (true, v),
        Err(_) => (false, 0),
    };

    unsafe { cb(ctx, is_visible, has_id, id) };
    true
}
