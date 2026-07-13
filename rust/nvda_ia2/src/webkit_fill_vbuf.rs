//! Port of `WebKitVBufBackend_t::fillVBuf` from
//! `nvdaHelper/vbufBackends/webKit/webKit.cpp:51`.
//!
//! WebKit's render is a much simpler cousin of gecko's: it walks the MSAA
//! `IAccessible` tree (role / state / name / value / description) using
//! only one IA2-specific call, `get_uniqueID`, for node identity. A single
//! `doc_handle` (the root document window) is threaded unchanged through
//! the whole recursion — WebKit never derives a per-node docHandle the way
//! gecko does. There is no cross-render node reuse: on invalidation the
//! shared `run_raw_update` orchestration re-renders the affected subtree
//! fresh into a temp buffer, exactly as the C++ backend relied on the base
//! `VBufBackend_t::update()` to do.

use crate::bstr::is_bstr_null;
use crate::interfaces::IAccessible2;
use nvda_vbuf::{VbufBuffer, VbufControlFieldNode, VbufFieldNode};
use windows::core::{Interface, VARIANT};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::UI::Accessibility::{AccessibleChildren, IAccessible};

// MSAA roles (oleacc.h).
const ROLE_SYSTEM_COLUMN: i32 = 0x1b;
const ROLE_SYSTEM_LIST: i32 = 0x21;
const ROLE_SYSTEM_TEXT: i32 = 0x2a;
const ROLE_SYSTEM_COMBOBOX: i32 = 0x2e;

// MSAA states (oleacc.h).
const STATE_SYSTEM_READONLY: i32 = 0x40;
const STATE_SYSTEM_FOCUSABLE: i32 = 0x100000;

// VARIANT discriminants (wtypes.h).
const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;

/// Decimal string of `n` as UTF-16, matching a `wostringstream << long`.
fn dec(n: i32) -> Vec<u16> {
    n.to_string().encode_utf16().collect()
}

/// Copy a raw `BSTR` pointer (from a VARIANT union) into an owned
/// `Vec<u16>` without taking ownership of the string. A BSTR stores its
/// byte length in the `u32` immediately preceding the character data.
/// Returns empty for a NULL pointer.
///
/// # Safety
///
/// `p` must be NULL or a valid `BSTR` (its length prefix and character
/// data readable). The returned copy is independent; the VARIANT retains
/// ownership and frees the original on `VariantClear`.
unsafe fn copy_raw_bstr(p: *const u16) -> Vec<u16> {
    if p.is_null() {
        return Vec::new();
    }
    let byte_len =
        unsafe { ((p as *const u8).sub(4) as *const u32).read_unaligned() };
    let len = (byte_len / 2) as usize;
    unsafe { core::slice::from_raw_parts(p, len) }.to_vec()
}

/// `(role_int, role_attr_string)` from the `get_accRole` VARIANT,
/// replicating the C++ if/else chain: VT_EMPTY -> "0" (role 0), VT_BSTR ->
/// the string (role 0), VT_I4 -> the decimal + the role value, anything
/// else (including a failed call, where the C++ `CComVariant` stays
/// VT_EMPTY) -> the VT_EMPTY branch's "0". A non-VT_I4/BSTR/EMPTY vt in
/// C++ leaves the stream empty -> "".
fn role_from_variant(var_role: &Option<VARIANT>) -> (i32, Vec<u16>) {
    let Some(v) = var_role else {
        // get_accRole failed: C++ leaves varRole default (VT_EMPTY) -> "0".
        return (0, dec(0));
    };
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_I4 {
        let l = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
        (l, dec(l))
    } else if vt == VT_BSTR {
        let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
        (0, unsafe { copy_raw_bstr(p) })
    } else if vt == VT_EMPTY {
        (0, dec(0))
    } else {
        (0, Vec::new())
    }
}

/// MSAA state bitmask from the `get_accState` VARIANT (VT_I4 payload, else
/// 0). Mirrors `int states = varState.lVal;` but guards the union read on
/// the discriminant the way gecko's port does.
fn states_from_variant(var_state: &Option<VARIANT>) -> i32 {
    let Some(v) = var_state else {
        return 0;
    };
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_I4 {
        unsafe { raw.Anonymous.Anonymous.Anonymous.lVal }
    } else {
        0
    }
}

/// Render `pacc` and its subtree into `buffer` under `parent_node` (after
/// `previous_node`). Returns the control field node created for `pacc`, or
/// `None` if the node was skipped (a WebKit column duplicate, a missing
/// unique ID, or an already-rendered identifier — the loop guard).
///
/// # Safety
///
/// `pacc` must be a valid `IAccessible2` for the duration; `buffer` and any
/// `Some` node handles must be live nodes in it. Must run on the render
/// thread with the backend lock held.
pub(crate) unsafe fn fill_vbuf(
    doc_handle: i32,
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous_node: Option<VbufFieldNode>,
) -> Option<VbufControlFieldNode> {
    let pacc_msaa: &IAccessible = pacc;
    // CHILDID_SELF for every accessor.
    let varchild = VARIANT::from(0i32);

    let var_role = unsafe { pacc_msaa.get_accRole(&varchild) }.ok();
    let (role, role_attr) = role_from_variant(&var_role);

    // WebKit exposes both a row and a column representation for tables,
    // duplicating every cell. Never take the column representation. `role`
    // is nonzero only for a VT_I4 role, so this already implies the C++
    // `varRole.vt == VT_I4` half of the check.
    if role == ROLE_SYSTEM_COLUMN {
        return None;
    }

    let id = match unsafe { pacc.get_uniqueID() } {
        Ok(i) => i,
        Err(_) => return None,
    };

    // Loop guard: bail if this identifier is already in the buffer.
    if unsafe {
        buffer.get_control_field_node_with_identifier(doc_handle, id)
    }
    .is_some()
    {
        return None;
    }

    let parent_node = unsafe {
        buffer.add_control_field_node(
            parent_node,
            previous_node,
            doc_handle,
            id,
            true,
        )
    }?;
    // From here `previous_node` tracks the last child appended under
    // `parent_node`; the caller's `previous_node` is consumed above.
    let mut previous_node: Option<VbufFieldNode> = None;

    unsafe {
        parent_node
            .as_field_node()
            .add_attribute(&utf16("IAccessible::role"), &role_attr);
    }

    // States: one `IAccessible::state_<bit>` attribute per set bit.
    let var_state = unsafe { pacc_msaa.get_accState(&varchild) }.ok();
    let states = states_from_variant(&var_state);
    for i in 0..32 {
        let state = 1i32 << i;
        if state & states != 0 {
            let name: Vec<u16> =
                format!("IAccessible::state_{state}").encode_utf16().collect();
            unsafe {
                parent_node
                    .as_field_node()
                    .add_attribute(&name, &[b'1' as u16]);
            }
        }
    }

    // Child count. Some interactive roles report content-less children we
    // never want, so force a leaf render for them.
    let suppress_children = role == ROLE_SYSTEM_COMBOBOX
        || (role == ROLE_SYSTEM_LIST && states & STATE_SYSTEM_READONLY == 0)
        || (role == ROLE_SYSTEM_TEXT && states & STATE_SYSTEM_FOCUSABLE != 0);
    let child_count = if suppress_children {
        0
    } else {
        unsafe { pacc_msaa.accChildCount() }.unwrap_or(0)
    };

    if child_count > 0 {
        let mut variants: Vec<VARIANT> =
            vec![VARIANT::default(); child_count as usize];
        let mut filled: i32 = 0;
        let res = unsafe {
            AccessibleChildren(pacc_msaa, 0, &mut variants[..], &mut filled)
        };
        if res.is_ok() {
            variants.truncate(filled as usize);
        } else {
            variants.clear();
        }
        for child in variants.iter() {
            let raw = child.as_raw();
            if unsafe { raw.Anonymous.Anonymous.vt } != VT_DISPATCH {
                continue;
            }
            let pdisp = unsafe { raw.Anonymous.Anonymous.Anonymous.pdispVal };
            if pdisp.is_null() {
                continue;
            }
            let pdisp_raw: *mut core::ffi::c_void = pdisp as *mut _;
            let pdisp_ref: &IDispatch =
                match IDispatch::from_raw_borrowed(&pdisp_raw) {
                    Some(p) => p,
                    None => continue,
                };
            let child_pacc = match pdisp_ref.cast::<IAccessible2>() {
                Ok(a) => a,
                Err(_) => continue,
            };
            if let Some(node) = unsafe {
                fill_vbuf(
                    doc_handle,
                    &child_pacc,
                    buffer,
                    Some(parent_node),
                    previous_node,
                )
            } {
                previous_node = Some(node.as_field_node());
            }
        }
        // Each VARIANT runs VariantClear on Vec drop, releasing pdispVal.
    } else {
        // Leaf: pull content from name, then value, then a prefixed
        // description, then a focusable-but-empty placeholder space.
        let mut content: Vec<u16> = Vec::new();

        // Name — skipped for focusable text fields and comboboxes.
        if (role != ROLE_SYSTEM_TEXT || states & STATE_SYSTEM_FOCUSABLE == 0)
            && role != ROLE_SYSTEM_COMBOBOX
        {
            if let Ok(b) = unsafe { pacc_msaa.get_accName(&varchild) } {
                if !is_bstr_null(&b) {
                    content = b.as_wide().to_vec();
                }
            }
        }
        if content.is_empty() {
            if let Ok(b) = unsafe { pacc_msaa.get_accValue(&varchild) } {
                if !is_bstr_null(&b) {
                    content = b.as_wide().to_vec();
                }
            }
        }
        if content.is_empty() {
            if let Ok(b) = unsafe { pacc_msaa.get_accDescription(&varchild) } {
                if !is_bstr_null(&b) {
                    // WebKit prefixes real descriptions with "Description: ";
                    // only such strings become content (the prefix stripped).
                    let w = b.as_wide();
                    let prefix = utf16("Description: ");
                    if w.len() >= prefix.len() && w[..prefix.len()] == prefix[..]
                    {
                        content = w[prefix.len()..].to_vec();
                    }
                }
            }
        }
        if content.is_empty() && states & STATE_SYSTEM_FOCUSABLE != 0 {
            // Focusable but empty: add a space so it stays reachable.
            content = vec![b' ' as u16];
        }
        if !content.is_empty() {
            unsafe {
                buffer.add_text_field_node(
                    Some(parent_node),
                    previous_node,
                    &content,
                );
            }
        }
    }

    Some(parent_node)
}

/// Encode an ASCII/UTF-8 `&str` as UTF-16.
fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
