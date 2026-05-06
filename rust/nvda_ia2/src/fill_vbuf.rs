//! Rust port of `GeckoVBufBackend_t::fillVBuf` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:408`.
//!
//! Carved into blocks per the design doc at
//! `docs/plans/2026-05-06-rust-fill-vbuf-design.md`. The single
//! C-callable entry point [`nvda_ia2_fill_vbuf`] is the integration
//! contract — it replaces the *body* of the C++ `fillVBuf` method on
//! the final flip. The C++ method shrinks to a one-liner that
//! converts member state to FFI args.
//!
//! Recursion stays Rust-side: [`fill_vbuf`] (the Rust internal
//! function) calls itself directly. The C++ side does not see the
//! recursive calls.
//!
//! Implementation status (block-by-block per the design doc):
//!
//! | Block | Title                              | Status |
//! | :---: | ---------------------------------- | ------ |
//! |   1   | entry, identity, IA2 attribs, role | done   |
//! |   2   | name / value / desc / locale / states | TODO |
//! |   3   | IA2 text segmentation              | TODO   |
//! |   4   | table state plumbing               | TODO   |
//! |   5   | non-text children walk             | TODO   |
//! |   6   | empty-content fallbacks            | TODO   |
//! |   7   | name attr / desc-is-content / aria | TODO   |
//!
//! Until all blocks land, [`fill_vbuf`] panics in the unimplemented
//! tail. The extern shim is therefore unsafe to wire into C++ until
//! every block is in place; gecko_ia2.cpp's `fillVBuf` keeps running
//! the C++ implementation at runtime in the meantime.
#![allow(dead_code)]

use core::ffi::c_void;
use std::collections::BTreeMap;

use crate::fetch::fetch_ia2_attributes_native;
use crate::interfaces::{IAccessible2, IAccessibleTable2};
use crate::role_long_string::get_role_long_role_string_native;
use nvda_vbuf::{VbufBackend, VbufBuffer, VbufControlFieldNode, VbufFieldNode};
use windows::core::Interface;

/// MSAA `ROLE_SYSTEM_OUTLINE` — listed in `oleacc.h` as `0x23` (35).
/// Hard-coded to avoid pulling another windows-rs feature in for a
/// single `i32`.
const ROLE_SYSTEM_OUTLINE: i32 = 0x23;
/// `ROLE_SYSTEM_TABLE` — `oleacc.h` value `0x18` (24).
const ROLE_SYSTEM_TABLE: i32 = 0x18;
/// `ROLE_SYSTEM_EQUATION` — `oleacc.h` value `0x35` (53).
const ROLE_SYSTEM_EQUATION: i32 = 0x35;
/// `ROLE_SYSTEM_GRAPHIC` — `oleacc.h` value `0x28` (40).
const ROLE_SYSTEM_GRAPHIC: i32 = 0x28;

/// Outcome of [`block1`] indicating how the caller should proceed.
pub enum Block1Outcome {
    /// `pacc` lacked a window handle, lacked a unique ID, or the
    /// (docHandle, ID) pair was already in the buffer (loop guard).
    /// `fillVBuf` should return null.
    Bail,
    /// Cross-buffer reuse fired: a reference node was added to
    /// `buffer` pointing at an existing node owned by `backend`.
    /// `fillVBuf` should return this node directly without
    /// continuing.
    Reused(VbufFieldNode),
    /// A new control field node was created and the IA2 attribute
    /// prelude (IAccessible2::attribute_*, IAccessible::role) is
    /// already populated. `fillVBuf` continues with the rest of the
    /// processing using the returned context.
    Continue(Block1Continue),
}

/// State produced by [`block1`] when processing should continue. All
/// fields mirror the C++ original's local variables at line 514, just
/// after the role attribute has been written.
pub struct Block1Continue {
    /// The new control field node added to the buffer. The C++ code
    /// re-uses the variable name `parentNode` for this past line 466.
    pub parent_node: VbufControlFieldNode,
    /// IA2 unique ID of `pacc`.
    pub id: i32,
    /// Document handle (truncated `HWND`).
    pub doc_handle: i32,
    /// Post-normalization IA2 role (treegrid override applied,
    /// equation-with-img-tag override applied).
    pub role: i32,
    /// The `roleAttr` C++ wstring -- either the BSTR from the MSAA
    /// fallback or a decimal stringification of `role`. Downstream
    /// code (notably `fillVBufAriaDetails`) consumes this.
    pub role_attr: Vec<u16>,
    /// Parsed IA2 attributes. Used downstream for many lookups
    /// (`display`, `formatting`, `tag`, `src`, etc.).
    pub attribs: BTreeMap<String, String>,
}

/// Block 1 of `fillVBuf`: get docHandle/ID, dedup, cross-buffer reuse,
/// add the new control field node, populate IA2 attributes, normalize
/// role, set the role attribute.
///
/// Mirrors lines 408-514 of `gecko_ia2.cpp` exactly — see the source
/// for the rationale behind each step.
///
/// # Safety
///
/// `pacc` must point at a live `IAccessible2`; `buffer`, `backend`,
/// `parent`, `previous` (if `Some`) must be live for the duration.
pub unsafe fn block1(
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    backend: VbufBackend,
    parent: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
) -> Block1Outcome {
    // Block 1 / step 1: docHandle from get_windowHandle. The C++
    // original truncates HWND to int via HandleToUlong; we follow.
    let hwnd = match unsafe { pacc.get_windowHandle() } {
        Ok(h) => h,
        Err(_) => return Block1Outcome::Bail,
    };
    let doc_handle: i32 = (hwnd.0 as usize) as i32;
    if doc_handle == 0 {
        return Block1Outcome::Bail;
    }

    // Block 1 / step 2: IA2 unique ID.
    let id = match unsafe { pacc.get_uniqueID() } {
        Ok(i) => i,
        Err(_) => return Block1Outcome::Bail,
    };

    // Block 1 / step 3: loop guard -- bail if already known.
    if unsafe {
        buffer.get_control_field_node_with_identifier(doc_handle, id)
    }
    .is_some()
    {
        return Block1Outcome::Bail;
    }

    // Block 1 / step 4: cross-buffer reuse. The C++ original gates
    // this on `buffer != this && parentNode`, where `this` is the
    // GeckoVBufBackend_t (which IS-A buffer). In Rust we compare the
    // wrapped pointers.
    if buffer.0 != backend.as_buffer().0 {
        if let Some(parent_node) = parent {
            if let Some(existing) = unsafe {
                backend.reuse_existing_node(
                    Some(parent_node),
                    previous,
                    doc_handle,
                    id,
                )
            } {
                // Mirrors the C++ early `return
                // buffer->addReferenceNodeToBuffer(...)`: whatever this
                // produces (including NULL) becomes fillVBuf's return.
                let reference = unsafe {
                    buffer.add_reference_node(
                        Some(parent_node),
                        previous,
                        existing,
                    )
                };
                return match reference {
                    Some(r) => Block1Outcome::Reused(r),
                    None => Block1Outcome::Bail,
                };
            }
        }
    }

    // Block 1 / step 5: add the new control field node. The C++ code
    // reassigns `parentNode` to the new node and resets `previousNode`
    // to NULL; we just produce the new control field node.
    let new_parent_node = match unsafe {
        buffer.add_control_field_node(parent, previous, doc_handle, id, true)
    } {
        Some(n) => n,
        None => {
            // The C++ original asserts non-NULL here (`nhAssert(parentNode)`).
            // On a debug build this would terminate the process; on a
            // release build it falls through to a NULL-deref shortly
            // after. We bail gracefully.
            return Block1Outcome::Bail;
        }
    };

    // Block 1 / step 6: IA2 attributes.
    let attribs = fetch_ia2_attributes_native(pacc);
    apply_ia2_attribs_to_node(new_parent_node, &attribs);

    // Block 1 / step 7: role + role normalization.
    let (mut role, role_string) =
        get_role_long_role_string_native(pacc, 0);
    if role == ROLE_SYSTEM_OUTLINE
        && has_xml_role_attrib_containing_value(&attribs, "treegrid")
    {
        role = ROLE_SYSTEM_TABLE;
    }
    if role == ROLE_SYSTEM_EQUATION
        && attribs.get("tag").map(|v| v.as_str()) == Some("img")
    {
        role = ROLE_SYSTEM_GRAPHIC;
    }

    // Block 1 / step 8: roleAttr -- BSTR from fallback or decimal role.
    let role_attr: Vec<u16> = match role_string {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            use core::fmt::Write;
            let _ = write!(buf, "{role}");
            buf.encode_utf16().collect()
        }
    };

    // Set IAccessible::role attribute.
    let role_name: Vec<u16> = "IAccessible::role".encode_utf16().collect();
    unsafe {
        new_parent_node
            .as_field_node()
            .add_attribute(&role_name, &role_attr);
    }

    Block1Outcome::Continue(Block1Continue {
        parent_node: new_parent_node,
        id,
        doc_handle,
        role,
        role_attr,
        attribs,
    })
}

/// Apply each `(key, val)` pair from the IA2 attributes map onto the
/// vbuf control field node as an `IAccessible2::attribute_<key>`
/// attribute. Mirrors the C++ loop at gecko_ia2.cpp:474-479.
fn apply_ia2_attribs_to_node(
    node: VbufControlFieldNode,
    attribs: &BTreeMap<String, String>,
) {
    const PREFIX: &str = "IAccessible2::attribute_";
    let prefix_u16: Vec<u16> = PREFIX.encode_utf16().collect();
    for (key, val) in attribs.iter() {
        let key_u16: Vec<u16> = key.encode_utf16().collect();
        let mut name = Vec::with_capacity(prefix_u16.len() + key_u16.len());
        name.extend_from_slice(&prefix_u16);
        name.extend_from_slice(&key_u16);
        let val_u16: Vec<u16> = val.encode_utf16().collect();
        unsafe {
            node.as_field_node().add_attribute(&name, &val_u16);
        }
    }
}

/// Port of `hasXmlRoleAttribContainingValue` from gecko_ia2.cpp:36.
/// Returns `true` if the IA2 attribute `xml-roles` is present and
/// contains `value` as a substring.
pub(crate) fn has_xml_role_attrib_containing_value(
    attribs: &BTreeMap<String, String>,
    value: &str,
) -> bool {
    attribs
        .get("xml-roles")
        .map(|v| v.contains(value))
        .unwrap_or(false)
}

/// Port of `hasAriaHiddenAttribute` from gecko_ia2.cpp.
/// Returns `true` if the IA2 attribute `hidden` is present and equals
/// `"true"` (case-sensitive, as the C++ original).
pub(crate) fn has_aria_hidden_attribute(
    attribs: &BTreeMap<String, String>,
) -> bool {
    attribs.get("hidden").map(|v| v.as_str()) == Some("true")
}

// ----- entry point ---------------------------------------------------

/// Per-render context the recursion threads through. Carries the
/// pieces of `GeckoVBufBackend_t` state that fillVBuf consults beyond
/// its formal arguments.
///
/// Constructed once at the C++ entry point, then borrowed by every
/// recursive Rust frame.
pub struct FillVBufCtx {
    /// The vbuf backend handle, used for cross-buffer reuse via
    /// [`VbufBackend::reuse_existing_node`]. Equivalent to `this` in
    /// the C++ original.
    pub backend: VbufBackend,
    /// The IA2 unique ID of the document root node. Used for the
    /// `isRoot` check (gecko_ia2.cpp:620). Equivalent to
    /// `this->rootID`.
    pub root_id: i32,
    /// `true` when the toolkit name is `"Chrome"`. Threaded down to
    /// `fill_vbuf_aria_details` / `fill_vbuf_aria_error` for the
    /// Chrome-specific `IAccessible2_2::get_relationTargetsOfType`
    /// workaround. Equivalent to `this->toolkitName == L"Chrome"`.
    pub is_chrome: bool,
}

/// Rust-internal entry point. Recursion uses this directly; the
/// extern C shim ([`nvda_ia2_fill_vbuf`]) constructs the inputs from
/// raw pointers and forwards.
///
/// `parent_pres_row_num` mirrors `parentPresentationalRowNumber` in
/// the C++ original — `None` means the parent does not propagate a
/// presentational row number; `Some(slice)` carries the wide-char
/// digits.
///
/// # Safety
///
/// * `pacc` must be live for the duration of the call.
/// * `buffer`, `parent`, `previous`, and `ctx.backend` must point at
///   live vbuf nodes / backends.
/// * `pacc_table2`, when `Some`, must be live for the duration.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn fill_vbuf(
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    parent: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    pacc_table2: Option<&IAccessibleTable2>,
    table_id: i32,
    parent_pres_row_num: Option<&[u16]>,
    ignore_interactive_unlabelled_graphics: bool,
    ctx: &FillVBufCtx,
) -> Option<VbufFieldNode> {
    // Block 1.
    let cont = match unsafe {
        block1(pacc, buffer, ctx.backend, parent, previous)
    } {
        Block1Outcome::Bail => return None,
        Block1Outcome::Reused(reference) => return Some(reference),
        Block1Outcome::Continue(c) => c,
    };

    // TODO Block 2 — name / value / description / locale / states.
    // TODO Block 3 — IA2 text segmentation loop.
    // TODO Block 4 — table state plumbing (uses pacc_table2 / table_id).
    // TODO Block 5 — AccessibleChildren recursion (calls back into
    //                fill_vbuf).
    // TODO Block 6 — graphic / progressbar / link content fallbacks.
    // TODO Block 7 — name-as-attribute, descriptionIsContent, calls to
    //                aria_details::fill_vbuf_aria_details and
    //                aria_error::fill_vbuf_aria_error using ctx.is_chrome.

    // Until those land, exercising the entry point is a bug; the C++
    // `fillVBuf` is the live implementation. Touch every parameter so
    // the unused-variable lints stay quiet without `#[allow]`.
    let _ = (
        cont,
        pacc_table2,
        table_id,
        parent_pres_row_num,
        ignore_interactive_unlabelled_graphics,
        ctx,
    );
    unimplemented!("fill_vbuf blocks 2-7 not yet ported")
}

/// Single C-callable entry point. Replaces the *body* of
/// `GeckoVBufBackend_t::fillVBuf` once every block is implemented.
/// The C++ method itself stays as a one-liner that unpacks `this`-
/// owned state into the FFI arguments.
///
/// Returns the resulting field node pointer, or `NULL` on bail.
/// Caller does not own the returned pointer; vbufBase manages node
/// lifetime through the buffer.
///
/// # Safety
///
/// * `pacc` must be a valid `IAccessible2*`; not consumed.
/// * `buffer` must be a valid `VBufStorage_buffer_t*`.
/// * `parent_node`, `previous_node`, `pacc_table2` may be `NULL` to
///   indicate "absent"; otherwise must be valid pointers of their
///   respective C++ types.
/// * `parent_pres_row_num_ptr` may be `NULL` (with `_len == 0`) for
///   absent; otherwise must point to `_len` valid `u16`s.
/// * `backend` must be a valid `VBufBackend_t*`.
/// * Caller (the C++ `fillVBuf` shim) must hold the render-thread
///   invariants vbufBase requires.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn nvda_ia2_fill_vbuf(
    pacc: *mut c_void,
    buffer: *mut c_void,
    parent_node: *mut c_void,
    previous_node: *mut c_void,
    pacc_table2: *mut c_void,
    table_id: i32,
    parent_pres_row_num_ptr: *const u16,
    parent_pres_row_num_len: usize,
    ignore_interactive_unlabelled_graphics: bool,
    backend: *mut c_void,
    root_id: i32,
    is_chrome: bool,
) -> *mut c_void {
    if pacc.is_null() || buffer.is_null() || backend.is_null() {
        return core::ptr::null_mut();
    }
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return core::ptr::null_mut(),
    };
    let table2: Option<&IAccessibleTable2> = if pacc_table2.is_null() {
        None
    } else {
        IAccessibleTable2::from_raw_borrowed(&pacc_table2)
    };

    let buffer = VbufBuffer(buffer);
    let backend = VbufBackend(backend);
    let parent = if parent_node.is_null() {
        None
    } else {
        Some(VbufControlFieldNode(parent_node))
    };
    let previous = if previous_node.is_null() {
        None
    } else {
        Some(VbufFieldNode(previous_node))
    };
    let pres_row: Option<&[u16]> =
        if parent_pres_row_num_ptr.is_null() || parent_pres_row_num_len == 0 {
            None
        } else {
            Some(unsafe {
                core::slice::from_raw_parts(
                    parent_pres_row_num_ptr,
                    parent_pres_row_num_len,
                )
            })
        };

    let ctx = FillVBufCtx {
        backend,
        root_id,
        is_chrome,
    };
    match unsafe {
        fill_vbuf(
            acc,
            buffer,
            parent,
            previous,
            table2,
            table_id,
            pres_row,
            ignore_interactive_unlabelled_graphics,
            &ctx,
        )
    } {
        Some(node) => node.0,
        None => core::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn xml_roles_substring_match() {
        let m = map_of(&[("xml-roles", "treegrid checkbox")]);
        assert!(has_xml_role_attrib_containing_value(&m, "treegrid"));
        assert!(has_xml_role_attrib_containing_value(&m, "checkbox"));
        // The C++ uses wstring::find -- a literal substring search,
        // so "tree" matches "treegrid" and "absent" does not.
        assert!(has_xml_role_attrib_containing_value(&m, "tree"));
        assert!(!has_xml_role_attrib_containing_value(&m, "absent"));
    }

    #[test]
    fn xml_roles_missing_attribute() {
        let m = map_of(&[]);
        assert!(!has_xml_role_attrib_containing_value(&m, "treegrid"));
    }

    #[test]
    fn aria_hidden_true() {
        let m = map_of(&[("hidden", "true")]);
        assert!(has_aria_hidden_attribute(&m));
    }

    #[test]
    fn aria_hidden_false_or_missing() {
        let m = map_of(&[("hidden", "false")]);
        assert!(!has_aria_hidden_attribute(&m));
        let empty = map_of(&[]);
        assert!(!has_aria_hidden_attribute(&empty));
    }
}
