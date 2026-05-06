//! Port of `GeckoVBufBackend_t::fillVBufAriaError` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:483`.
//!
//! Looks up the IA2 `"error"` relation target on the given IAccessible2,
//! extracts text from it via `getTextFromIAccessible`, and stores it on
//! the vbuf node's `errorMessage` attribute.

use windows::core::Interface;

use crate::interfaces::{IAccessible2, IAccessible2_2};
use crate::relation_targets::get_relation_targets_of_type_native;
use crate::text::get_text_from_iaccessible_collect;
use nvda_vbuf::VbufFieldNode;

/// IA2 relation name `"error"` (`IA2_RELATION_ERROR` from
/// `AccessibleRelation.idl:170`).
const IA2_RELATION_ERROR: &[u16] = &[
    b'e' as u16,
    b'r' as u16,
    b'r' as u16,
    b'o' as u16,
    b'r' as u16,
];

const ATTR_NAME_ERROR_MESSAGE: &[u16] = &[
    b'e' as u16,
    b'r' as u16,
    b'r' as u16,
    b'o' as u16,
    b'r' as u16,
    b'M' as u16,
    b'e' as u16,
    b's' as u16,
    b's' as u16,
    b'a' as u16,
    b'g' as u16,
    b'e' as u16,
];

/// Rust-native variant for in-crate callers (the fillVBuf port).
///
/// # Safety
///
/// `pacc` must be live; `node` must be a live control field node.
pub(crate) unsafe fn fill_vbuf_aria_error_native(
    pacc: &IAccessible2,
    node: VbufFieldNode,
    is_chrome: bool,
) {
    let acc2_2: IAccessible2_2 = match pacc.cast() {
        Ok(a) => a,
        Err(_) => return,
    };

    let targets = get_relation_targets_of_type_native(
        &acc2_2,
        IA2_RELATION_ERROR,
        1,
        is_chrome,
    );
    let target = match targets.into_iter().next().flatten() {
        Some(t) => t,
        None => return,
    };

    let mut text_buf: Vec<u16> = Vec::new();
    let got_text = get_text_from_iaccessible_collect(
        &mut text_buf,
        &target,
        false,
        true,
        true,
    );
    if !got_text {
        return;
    }

    unsafe {
        node.add_attribute(ATTR_NAME_ERROR_MESSAGE, &text_buf);
    }
}

/// C-callable replacement.
///
/// `pacc` is the source IAccessible2 (borrowed). `node` is the
/// `VBufStorage_controlFieldNode_t*` to set `errorMessage` on.
/// `is_chrome` is `true` when the toolkit is Chrome (workaround for
/// the `max_targets`-ignoring bug in Chrome's IA2 implementation).
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*` for the duration.
/// * `node` must be a valid `VBufStorage_controlFieldNode_t*` for the
///   duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_fill_vbuf_aria_error(
    pacc: *mut core::ffi::c_void,
    node: *mut core::ffi::c_void,
    is_chrome: bool,
) {
    if pacc.is_null() || node.is_null() {
        return;
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return,
    };
    unsafe {
        fill_vbuf_aria_error_native(acc, VbufFieldNode(node), is_chrome);
    }
}
