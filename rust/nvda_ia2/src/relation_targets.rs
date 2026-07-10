//! Port of `GeckoVBufBackend_t::getRelationElementsOfType` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:52-117`.
//!
//! Calls `IAccessible2_2::get_relationTargetsOfType` and walks the
//! resulting `IUnknown**` array, QI-ing each entry to `IAccessible2`
//! and collecting them in order. Releases the CoTaskMem-allocated
//! outer array on completion.
//!
//! The C++ original released the outer array but never released the
//! IUnknowns past `numToReturn` (a long-standing leak; same shape as
//! `Ht2HyperlinkGetter::~Ht2HyperlinkGetter`). This Rust port fixes the
//! leak: every entry is taken ownership of via `core::ptr::read`, the
//! first `num_to_return` are returned to the caller, the rest are
//! dropped (releasing).

use crate::interfaces::{IAccessible2, IAccessible2_2};
use windows::core::{IUnknown, Interface, BSTR};
use windows::Win32::System::Com::CoTaskMemFree;

/// Rust-native variant of `getRelationElementsOfType`. Returns the
/// targets in the same shape as the C++ original: a `Vec` whose entries
/// are `Some(acc)` for each successful QI to `IAccessible2`, or `None`
/// where the COM server returned a slot but the QI failed (mirroring
/// the C++ vector-of-`CComQIPtr` push of a null entry).
///
/// Returns an empty `Vec` on any COM failure.
///
/// # Safety
///
/// `acc` must be a valid `IAccessible2_2`. (Holding a `&` is sufficient
/// — `windows-rs` lifts the AddRef/Release contract.)
pub fn get_relation_targets_of_type_native(
    acc: &IAccessible2_2,
    relation: &[u16],
    max_targets: i32,
    is_chrome: bool,
) -> Vec<Option<IAccessible2>> {
    let relation_bstr = match BSTR::from_wide(relation) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let effective_max = if is_chrome { 0 } else { max_targets };

    let (raw_targets, count) = match unsafe {
        acc.get_relationTargetsOfType(&relation_bstr, effective_max)
    } {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let count = count.max(0) as usize;
    if raw_targets.is_null() || count == 0 {
        unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
        return Vec::new();
    }

    let num_to_return = if max_targets <= 0 {
        count
    } else {
        count.min(max_targets as usize)
    };

    let mut out: Vec<Option<IAccessible2>> = Vec::with_capacity(num_to_return);
    for i in 0..count {
        let raw_iunk = unsafe { core::ptr::read(raw_targets.add(i)) };
        if i >= num_to_return {
            if !raw_iunk.is_null() {
                drop(unsafe { IUnknown::from_raw(raw_iunk) });
            }
            continue;
        }
        if raw_iunk.is_null() {
            out.push(None);
            continue;
        }
        let unk: IUnknown = unsafe { IUnknown::from_raw(raw_iunk) };
        out.push(unk.cast::<IAccessible2>().ok());
    }

    unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
    out
}
