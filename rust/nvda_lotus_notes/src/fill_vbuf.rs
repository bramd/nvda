//! Port of the render logic from
//! `nvdaHelper/vbufBackends/lotusNotesRichText/lotusNotesRichText.cpp`:
//! `render` (root resolution + child enumeration) and
//! `renderControlContent` (per-child node).
//!
//! Everything runs off a single client `IAccessible` obtained from the
//! document window via `WM_GETOBJECT`. Children are MSAA simple children
//! (VT_I4 child IDs), so the whole tree is two levels deep and there is no
//! recursion — each child becomes one control node carrying a single text
//! node (its value, or name, or a placeholder space).

use core::ffi::c_void;

use nvda_vbuf::{VbufBuffer, VbufControlFieldNode, VbufFieldNode};
use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::{
    AccessibleChildren, IAccessible, ObjectFromLresult,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, OBJID_CLIENT, SMTO_ABORTIFHUNG, WM_GETOBJECT,
};

// MSAA role (oleacc.h). ROLE_SYSTEM_TEXT suppresses using the name as
// content; ROLE_SYSTEM_CLIENT is the fixed role of the synthetic root.
const ROLE_SYSTEM_TEXT: i32 = 0x2a;
const ROLE_SYSTEM_CLIENT: &str = "10";

// VARIANT discriminants (wtypes.h).
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;

/// Decimal string of `n` as UTF-16 (a `wostringstream << long`).
fn dec(n: i32) -> Vec<u16> {
    n.to_string().encode_utf16().collect()
}

/// UTF-16 encode an ASCII/UTF-8 `&str`.
fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Copy a raw `BSTR` pointer (from a VARIANT union) to an owned `Vec<u16>`
/// without taking ownership — the VARIANT frees it on `VariantClear`. A
/// BSTR stores its byte length in the `u32` before the character data.
///
/// # Safety
///
/// `p` must be NULL or a valid `BSTR`.
unsafe fn copy_raw_bstr(p: *const u16) -> Vec<u16> {
    if p.is_null() {
        return Vec::new();
    }
    let byte_len =
        unsafe { ((p as *const u8).sub(4) as *const u32).read_unaligned() };
    let len = (byte_len / 2) as usize;
    unsafe { core::slice::from_raw_parts(p, len) }.to_vec()
}

/// Resolve the document window's client `IAccessible`, mirroring the top
/// of C++ `render`: `WM_GETOBJECT` (sent directly, bypassing proxying) ->
/// `ObjectFromLresult`. Returns `None` if the window doesn't answer or
/// doesn't support `IAccessible`.
///
/// # Safety
///
/// `doc_handle` is reinterpreted as an `HWND` (the C++
/// `(HWND)UlongToHandle(docHandle)` cast) and sent a window message; the
/// COM apartment must be initialised.
pub(crate) unsafe fn resolve_client_iaccessible(
    doc_handle: i32,
) -> Option<IAccessible> {
    use windows::Win32::Foundation::HWND;
    let hwnd = HWND(doc_handle as isize as *mut c_void);
    let mut res: usize = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETOBJECT,
            WPARAM(0),
            LPARAM(OBJID_CLIENT.0 as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut res),
        )
    };
    if sent.0 == 0 || res == 0 {
        return None;
    }
    let mut ppv: *mut c_void = core::ptr::null_mut();
    if unsafe {
        ObjectFromLresult(LRESULT(res as isize), &IAccessible::IID, WPARAM(0), &mut ppv)
    }
    .is_err()
        || ppv.is_null()
    {
        return None;
    }
    Some(unsafe { IAccessible::from_raw(ppv) })
}

/// Port of `render`'s `ID == 0` branch: build the synthetic client root
/// (docHandle, 0) and enumerate its MSAA children, rendering each VT_I4
/// child through [`render_control_content`].
///
/// # Safety
///
/// `pacc` must be a valid client `IAccessible`; `buffer` must be live.
pub(crate) unsafe fn render_root(
    doc_handle: i32,
    pacc: &IAccessible,
    buffer: VbufBuffer,
) {
    let Some(root) = (unsafe {
        buffer.add_control_field_node(None, None, doc_handle, 0, true)
    }) else {
        return;
    };
    unsafe {
        root.as_field_node()
            .add_attribute(&utf16("IAccessible::role"), &utf16(ROLE_SYSTEM_CLIENT));
    }

    let child_count = unsafe { pacc.accChildCount() }.unwrap_or(0);
    if child_count <= 0 {
        return;
    }
    let mut variants: Vec<VARIANT> =
        vec![VARIANT::default(); child_count as usize];
    let mut filled: i32 = 0;
    let res =
        unsafe { AccessibleChildren(pacc, 0, &mut variants[..], &mut filled) };
    if res.is_ok() {
        variants.truncate(filled as usize);
    } else {
        variants.clear();
    }

    let mut previous: Option<VbufFieldNode> = None;
    for child in variants.iter() {
        let raw = child.as_raw();
        if unsafe { raw.Anonymous.Anonymous.vt } != VT_I4 {
            continue;
        }
        let child_id = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
        // Faithful to the C++ `previousNode = renderControlContent(...)`:
        // the assignment is unconditional, so a skipped (None) child
        // resets `previous` to None.
        previous = unsafe {
            render_control_content(
                doc_handle,
                pacc,
                child_id,
                buffer,
                Some(root),
                previous,
            )
        }
        .map(|n| n.as_field_node());
    }
}

/// Port of `renderControlContent`: add one control node for the MSAA child
/// `acc_child_id` (queried off the client `pacc`), its role/state
/// attributes, and a single text node holding its value / name / a space.
/// Returns the created control node, or `None` if the identifier was
/// already in the buffer (the loop guard).
///
/// # Safety
///
/// `pacc` must be a valid client `IAccessible`; `buffer` and any `Some`
/// node handles must be live nodes in it.
pub(crate) unsafe fn render_control_content(
    doc_handle: i32,
    pacc: &IAccessible,
    acc_child_id: i32,
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous_node: Option<VbufFieldNode>,
) -> Option<VbufControlFieldNode> {
    let varchild = VARIANT::from(acc_child_id);
    let id = acc_child_id;

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
    let previous_node: Option<VbufFieldNode> = None;

    // Role: failure -> "0"; VT_BSTR -> the string (role 0); VT_I4 -> the
    // decimal + the role value; any other vt -> "" (empty stream).
    let (role, role_attr): (i32, Vec<u16>) =
        match unsafe { pacc.get_accRole(&varchild) } {
            Err(_) => (0, dec(0)),
            Ok(v) => {
                let raw = v.as_raw();
                let vt = unsafe { raw.Anonymous.Anonymous.vt };
                if vt == VT_I4 {
                    let l = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
                    (l, dec(l))
                } else if vt == VT_BSTR {
                    let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
                    (0, unsafe { copy_raw_bstr(p) })
                } else {
                    (0, Vec::new())
                }
            }
        };
    unsafe {
        parent_node
            .as_field_node()
            .add_attribute(&utf16("IAccessible::role"), &role_attr);
    }

    // States: one `IAccessible::state_<bit>` attribute per set bit. On a
    // failed call the C++ treats the state mask as 0.
    let states: i32 = match unsafe { pacc.get_accState(&varchild) } {
        Ok(v) => {
            let raw = v.as_raw();
            if unsafe { raw.Anonymous.Anonymous.vt } == VT_I4 {
                unsafe { raw.Anonymous.Anonymous.Anonymous.lVal }
            } else {
                0
            }
        }
        Err(_) => 0,
    };
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

    // Content: value wins; else the name (unless this is a plain text
    // role); else a placeholder space so the node stays reachable.
    let name = match unsafe { pacc.get_accName(&varchild) } {
        Ok(b) => b.as_wide().to_vec(),
        Err(_) => Vec::new(),
    };
    let value = match unsafe { pacc.get_accValue(&varchild) } {
        Ok(b) => b.as_wide().to_vec(),
        Err(_) => Vec::new(),
    };
    let content: Vec<u16> = if !value.is_empty() {
        value
    } else if role != ROLE_SYSTEM_TEXT && !name.is_empty() {
        name
    } else {
        vec![b' ' as u16]
    };
    if !content.is_empty() {
        unsafe {
            buffer.add_text_field_node(
                Some(parent_node),
                previous_node,
                &content,
            );
        }
    }

    Some(parent_node)
}
