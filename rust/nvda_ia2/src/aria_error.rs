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
    let acc2_2: IAccessible2_2 = match acc.cast() {
        Ok(a) => a,
        Err(_) => return,
    };

    let targets = get_relation_targets_of_type_native(
        &acc2_2,
        IA2_RELATION_ERROR,
        1,
        is_chrome,
    );
    // `aria-errormessage` is an ID reference, so only the first target
    // matters. The C++ original early-returned on empty; we do the same
    // and additionally bail if the QI to IAccessible2 failed (the C++
    // version checked `target != nullptr` against a possibly-null
    // CComQIPtr push).
    let target = match targets.into_iter().next().flatten() {
        Some(t) => t,
        None => return,
    };

    let mut text_buf: Vec<u16> = Vec::new();
    let got_text = get_text_from_iaccessible_collect(
        &mut text_buf,
        &target,
        false, // use_new_text
        true,  // recurse
        true,  // include_top_level_text
    );
    if !got_text {
        return;
    }

    let field_node = VbufFieldNode(node);
    unsafe {
        field_node.add_attribute(ATTR_NAME_ERROR_MESSAGE, &text_buf);
    }
}
