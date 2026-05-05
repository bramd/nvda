//! Port of `GeckoVBufBackend_t::getRelationElementsOfType` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:52-117`.
//!
//! Calls `IAccessible2_2::get_relationTargetsOfType` and walks the
//! resulting `IUnknown**` array, QI-ing each entry to `IAccessible2`
//! and forwarding to a callback in order. Releases the CoTaskMem-
//! allocated outer array on completion.
//!
//! The C++ original released the outer array but never released the
//! IUnknowns past `numToReturn` (a long-standing leak; same shape as
//! `Ht2HyperlinkGetter::~Ht2HyperlinkGetter`). This Rust port fixes the
//! leak: every entry is taken ownership of via `core::ptr::read`, the
//! first `num_to_return` are forwarded to the callback (transferring
//! the AddRef'd reference to the C++ caller), the rest are dropped
//! (releasing).

use crate::interfaces::{IAccessible2, IAccessible2_2};
use windows::core::{IUnknown, Interface, BSTR};
use windows::Win32::System::Com::CoTaskMemFree;

/// C-callable callback invoked once per target. `iaccessible2_ptr` is
/// either `null` (QI to IAccessible2 failed -- caller should treat as
/// missing) or an AddRef'd `IAccessible2*` (caller `Release`s).
pub type RelationTargetCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    iaccessible2_ptr: *mut core::ffi::c_void,
);

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

/// C-callable replacement for `getRelationElementsOfType`.
///
/// `pacc2_2` is borrowed (no `Release`). `relation_ptr` + `relation_len`
/// describe the IA2 relation type string (e.g. the `IA2_RELATION_*`
/// constants). `max_targets` is the maximum number of targets to forward
/// to the callback; pass `0` for "all". `is_chrome` enables the Chrome
/// workaround (force a 0/all request to the COM call to avoid
/// <https://crbug.com/1399184>); the post-fetch limit still applies.
///
/// Returns `true` on success (callback may have been invoked zero or
/// more times). Returns `false` on any COM failure.
///
/// # Safety
///
/// * `pacc2_2` must be a valid `IAccessible2_2*` for the duration.
/// * `relation_ptr` must point to `relation_len` valid `u16`s.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind. C++ exceptions thrown out of the callback
///   would propagate through the `extern "C"` frame, which is undefined
///   behavior on stable Rust.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_relation_targets_of_type(
    pacc2_2: *mut core::ffi::c_void,
    relation_ptr: *const u16,
    relation_len: usize,
    max_targets: i32,
    is_chrome: bool,
    ctx: *mut core::ffi::c_void,
    cb: RelationTargetCallback,
) -> bool {
    if pacc2_2.is_null() || relation_ptr.is_null() {
        return false;
    }
    let acc: &IAccessible2_2 =
        match IAccessible2_2::from_raw_borrowed(&pacc2_2) {
            Some(a) => a,
            None => return false,
        };

    // Build a BSTR from the (relation_ptr, relation_len) slice.
    let relation_slice =
        unsafe { core::slice::from_raw_parts(relation_ptr, relation_len) };
    let relation_bstr = match BSTR::from_wide(relation_slice) {
        Ok(b) => b,
        Err(_) => return false,
    };

    // Chrome workaround: max_targets is not respected by Chrome's
    // implementation, so request all (effective_max=0) and rely on the
    // post-fetch numToReturn cap. See crbug.com/1399184.
    let effective_max = if is_chrome { 0 } else { max_targets };

    let (raw_targets, count) =
        match unsafe { acc.get_relationTargetsOfType(&relation_bstr, effective_max) } {
            Ok(v) => v,
            Err(_) => return false,
        };
    let count = count.max(0) as usize;
    if raw_targets.is_null() || count == 0 {
        // CoTaskMemFree on null is documented as a no-op; call it
        // unconditionally for symmetry with the success path.
        unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
        return true;
    }

    // How many entries to forward; remainder are released.
    let num_to_return = if max_targets <= 0 {
        count
    } else {
        count.min(max_targets as usize)
    };

    for i in 0..count {
        // Take ownership of this slot's IUnknown ref. The COM server
        // AddRef'd each entry (per the IDL out-array contract); we
        // consume that ref via from_raw.
        let raw_iunk = unsafe { core::ptr::read(raw_targets.add(i)) };
        if i >= num_to_return {
            if !raw_iunk.is_null() {
                let unk: IUnknown = unsafe { IUnknown::from_raw(raw_iunk) };
                drop(unk);
            }
            continue;
        }
        let acc2_raw: *mut core::ffi::c_void = if raw_iunk.is_null() {
            core::ptr::null_mut()
        } else {
            let unk: IUnknown = unsafe { IUnknown::from_raw(raw_iunk) };
            match unk.cast::<IAccessible2>() {
                // into_raw transfers ownership of the AddRef'd pointer
                // to the caller; no Release here.
                Ok(acc2) => acc2.into_raw(),
                // QI failed: drop releases the underlying IUnknown ref;
                // forward a null to the callback so its semantics match
                // the C++ original (which would push a null CComQIPtr
                // entry into the result vector).
                Err(_) => core::ptr::null_mut(),
            }
        };
        unsafe { cb(ctx, acc2_raw) };
    }

    unsafe { CoTaskMemFree(Some(raw_targets as *const _)) };
    true
}
