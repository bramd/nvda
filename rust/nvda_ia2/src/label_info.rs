//! Port of `GeckoVBufBackend_t::getLabelInfo` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:274`.
//!
//! Resolves an `IA2_RELATION_LABELLED_BY` relation to its first target,
//! checks whether the label element is visible and has accessible text
//! content, and returns the target's IA2 unique ID. Used by the gecko
//! vbuf backend during render to decide whether to surface a label.

use crate::interfaces::{IAccessible2, IAccessible2_2};
use crate::text::get_text_from_iaccessible_collect;
use crate::utf16::utf16;
use windows::core::{IUnknown, Interface, BSTR, VARIANT};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Accessibility::IAccessible;
use windows::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;

/// Wide-string spelling of the `IA2_RELATION_LABELLED_BY` constant from
/// `include/ia2/api/AccessibleRelation.idl:119`.
const LABELLED_BY: &[u16] = &utf16(b"labelledBy");

/// Resolved label info, mirroring the C++ `LabelInfo` struct.
pub struct LabelInfo {
    pub is_visible: bool,
    pub id: Option<i32>,
}

/// Rust-native variant of `GeckoVBufBackend_t::getLabelInfo` for
/// in-crate callers. Returns `Some(LabelInfo)` when a labelling
/// relation was found; `None` otherwise.
///
/// # Safety
///
/// `pacc` must be a valid, live `IAccessible2`.
pub(crate) unsafe fn get_label_info_native(
    pacc: &IAccessible2,
) -> Option<LabelInfo> {
    let acc2_2: IAccessible2_2 = pacc.cast().ok()?;
    let relation_bstr = BSTR::from_wide(LABELLED_BY).ok()?;
    let (raw_targets, count) =
        unsafe { acc2_2.get_relationTargetsOfType(&relation_bstr, 1) }.ok()?;
    let count = count.max(0) as usize;
    if raw_targets.is_null() || count == 0 {
        unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
        return None;
    }

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
        return None;
    }
    let first_unk: IUnknown = unsafe { IUnknown::from_raw(first_raw) };
    let first_acc: IAccessible2 = first_unk.cast().ok()?;

    let pacc_first: &IAccessible = &first_acc;
    let varchild = VARIANT::from(0i32);
    let mut is_visible = false;
    if let Ok(state) = unsafe { pacc_first.get_accState(&varchild) } {
        let raw = state.as_raw();
        let lval = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
        is_visible = (lval & (STATE_SYSTEM_INVISIBLE.0 as i32)) == 0;
    }
    if is_visible {
        let mut text_buf: Vec<u16> = Vec::new();
        let got_text = get_text_from_iaccessible_collect(
            &mut text_buf,
            &first_acc,
            false,
            true,
            true,
        );
        if !got_text {
            is_visible = false;
        }
    }

    let id = unsafe { first_acc.get_uniqueID() }.ok();

    Some(LabelInfo { is_visible, id })
}
