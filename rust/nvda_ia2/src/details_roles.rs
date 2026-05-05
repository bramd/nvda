//! Port of `_extendDetailsRolesAttribute` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:439`.
//!
//! Appends `details_role` to a comma-separated `detailsRoles`
//! attribute on the given vbuf control field node, creating the
//! attribute if it doesn't yet exist.
//!
//! First user of the `nvda_vbuf` shim layer.

use nvda_vbuf::VbufFieldNode;

/// `detailsRoles` as a UTF-16 slice. The attribute name doesn't change.
const ATTR_NAME_DETAILS_ROLES: &[u16] = &[
    b'd' as u16,
    b'e' as u16,
    b't' as u16,
    b'a' as u16,
    b'i' as u16,
    b'l' as u16,
    b's' as u16,
    b'R' as u16,
    b'o' as u16,
    b'l' as u16,
    b'e' as u16,
    b's' as u16,
];

/// C-callable replacement. `node` is a `VBufStorage_controlFieldNode_t*`;
/// `role_ptr` + `role_len` describe the role-name string to append.
///
/// # Safety
///
/// * `node` must be a valid `VBufStorage_controlFieldNode_t*` for the
///   duration of the call.
/// * `role_ptr` must point to `role_len` valid `u16`s.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_extend_details_roles_attribute(
    node: *mut core::ffi::c_void,
    role_ptr: *const u16,
    role_len: usize,
) {
    if node.is_null() || (role_len > 0 && role_ptr.is_null()) {
        return;
    }
    let role = unsafe { core::slice::from_raw_parts(role_ptr, role_len) };
    unsafe { extend_details_roles_attribute(VbufFieldNode(node), role) };
}

/// Rust-side variant for callers that already hold a `VbufFieldNode`
/// handle (e.g. the `aria_details` port).
///
/// # Safety
///
/// `node` must be a live control field node.
pub(crate) unsafe fn extend_details_roles_attribute(
    node: VbufFieldNode,
    role: &[u16],
) {
    let existing = unsafe { node.get_attribute(ATTR_NAME_DETAILS_ROLES) };
    let new_value = combine_details_roles(existing.as_deref(), role);
    // `addAttribute` replaces an attribute that already exists, so a
    // single call covers both the "extend" and "create" cases.
    unsafe {
        node.add_attribute(ATTR_NAME_DETAILS_ROLES, &new_value);
    }
}

/// Build the new value for the `detailsRoles` attribute.
/// `None` for `existing` means the attribute is absent; otherwise the
/// existing value is extended with `,role`.
fn combine_details_roles(existing: Option<&[u16]>, role: &[u16]) -> Vec<u16> {
    match existing {
        Some(existing) => {
            let mut out = Vec::with_capacity(existing.len() + 1 + role.len());
            out.extend_from_slice(existing);
            out.push(',' as u16);
            out.extend_from_slice(role);
            out
        }
        None => role.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::combine_details_roles;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn no_existing_attribute_writes_role_alone() {
        assert_eq!(combine_details_roles(None, &w("comment")), w("comment"));
    }

    #[test]
    fn existing_attribute_appends_with_comma() {
        let existing = w("comment");
        assert_eq!(
            combine_details_roles(Some(&existing), &w("definition")),
            w("comment,definition")
        );
    }

    #[test]
    fn empty_existing_keeps_leading_comma() {
        // Matches the C++ wstringstream behavior: empty existing still
        // emits the comma separator.
        assert_eq!(
            combine_details_roles(Some(&[]), &w("comment")),
            w(",comment")
        );
    }

    #[test]
    fn multiple_appends_preserve_order() {
        let v1 = combine_details_roles(None, &w("a"));
        let v2 = combine_details_roles(Some(&v1), &w("b"));
        let v3 = combine_details_roles(Some(&v2), &w("c"));
        assert_eq!(v3, w("a,b,c"));
    }
}
