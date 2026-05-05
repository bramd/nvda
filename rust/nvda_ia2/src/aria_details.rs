//! Port of `GeckoVBufBackend_t::fillVBufAriaDetails` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:432`.
//!
//! Sets `hasDetails="true"` plus a `detailsRoles` list on the node when
//! the source IAccessible2 is the *origin* of one or more
//! `IA2_RELATION_DETAILS` relations, and extends `detailsRoles` on each
//! origin node when the source is the *target* (via
//! `IA2_RELATION_DETAILS_FOR`).

use windows::core::Interface;

use crate::details_roles::extend_details_roles_attribute;
use crate::interfaces::{IAccessible2, IAccessible2_2};
use crate::relation_targets::get_relation_targets_of_type_native;
use nvda_vbuf::{VbufBuffer, VbufControlFieldNode};

/// `IA2_RELATION_DETAILS` from `AccessibleRelation.idl:163`.
const IA2_RELATION_DETAILS: &[u16] = &[
    b'd' as u16,
    b'e' as u16,
    b't' as u16,
    b'a' as u16,
    b'i' as u16,
    b'l' as u16,
    b's' as u16,
];

/// `IA2_RELATION_DETAILS_FOR` from `AccessibleRelation.idl:167`.
const IA2_RELATION_DETAILS_FOR: &[u16] = &[
    b'd' as u16,
    b'e' as u16,
    b't' as u16,
    b'a' as u16,
    b'i' as u16,
    b'l' as u16,
    b's' as u16,
    b'F' as u16,
    b'o' as u16,
    b'r' as u16,
];

const ATTR_NAME_HAS_DETAILS: &[u16] = &[
    b'h' as u16,
    b'a' as u16,
    b's' as u16,
    b'D' as u16,
    b'e' as u16,
    b't' as u16,
    b'a' as u16,
    b'i' as u16,
    b'l' as u16,
    b's' as u16,
];

const VAL_TRUE: &[u16] =
    &[b't' as u16, b'r' as u16, b'u' as u16, b'e' as u16];

const ATTR_NAME_ROLE: &[u16] =
    &[b'r' as u16, b'o' as u16, b'l' as u16, b'e' as u16];

const VAL_UNKNOWN: &[u16] = &[
    b'u' as u16,
    b'n' as u16,
    b'k' as u16,
    b'n' as u16,
    b'o' as u16,
    b'w' as u16,
    b'n' as u16,
];

/// Fetch the list of IA2 unique IDs for every target of `relation` on
/// `acc2_2`. Mirrors `getAllRelationIdsForRelationType` in
/// gecko_ia2.cpp:267.
fn get_all_relation_ids(
    acc2_2: &IAccessible2_2,
    relation: &[u16],
    is_chrome: bool,
) -> Vec<i32> {
    let targets =
        get_relation_targets_of_type_native(acc2_2, relation, 0, is_chrome);
    let mut ids = Vec::with_capacity(targets.len());
    for t in targets.into_iter().flatten() {
        if let Ok(id) = unsafe { t.get_uniqueID() } {
            ids.push(id);
        }
    }
    ids
}

/// C-callable replacement for `fillVBufAriaDetails`.
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*` for the duration.
/// * `buffer` must be a valid `VBufStorage_buffer_t*`.
/// * `node` must be a valid `VBufStorage_controlFieldNode_t*`.
/// * `node_role_ptr` + `node_role_len` must describe a readable UTF-16
///   slice (or `node_role_len == 0` for an empty role).
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_fill_vbuf_aria_details(
    doc_handle: i32,
    pacc: *mut core::ffi::c_void,
    buffer: *mut core::ffi::c_void,
    node: *mut core::ffi::c_void,
    node_role_ptr: *const u16,
    node_role_len: usize,
    is_chrome: bool,
) {
    if pacc.is_null() || buffer.is_null() || node.is_null() {
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

    let buffer = VbufBuffer(buffer);
    let node_being_filled = VbufControlFieldNode(node);
    let node_role: &[u16] = if node_role_len == 0 || node_role_ptr.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(node_role_ptr, node_role_len) }
    };

    // Origin case: nodeBeingFilled has DETAILS targets.
    let detail_target_ids =
        get_all_relation_ids(&acc2_2, IA2_RELATION_DETAILS, is_chrome);
    if !detail_target_ids.is_empty() {
        unsafe {
            node_being_filled
                .as_field_node()
                .add_attribute(ATTR_NAME_HAS_DETAILS, VAL_TRUE);
        }
        for id in &detail_target_ids {
            let target_node = match unsafe {
                buffer.get_control_field_node_with_identifier(doc_handle, *id)
            } {
                Some(n) => n,
                None => continue,
            };
            let target_role = unsafe {
                target_node.as_field_node().get_attribute(ATTR_NAME_ROLE)
            };
            // If the target has no role attribute, fall back to "unknown" so
            // that "hasDetails" with multiple relations stays informative
            // even when one target's role is generic.
            let role: &[u16] = match target_role.as_deref() {
                Some(r) => r,
                None => VAL_UNKNOWN,
            };
            unsafe {
                extend_details_roles_attribute(
                    node_being_filled.as_field_node(),
                    role,
                );
            }
        }
    }

    // Target case: nodeBeingFilled is the target of DETAILS_FOR origins.
    let detail_origin_ids =
        get_all_relation_ids(&acc2_2, IA2_RELATION_DETAILS_FOR, is_chrome);
    for id in &detail_origin_ids {
        let origin_node = match unsafe {
            buffer.get_control_field_node_with_identifier(doc_handle, *id)
        } {
            Some(n) => n,
            None => continue,
        };
        unsafe {
            extend_details_roles_attribute(
                origin_node.as_field_node(),
                node_role,
            );
        }
    }
}
