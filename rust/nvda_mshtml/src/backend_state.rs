//! Per-instance Rust state + C-ABI entry points for
//! `MshtmlVBufBackend_t` (Phase A), mirroring
//! `nvda_acrobat::backend_state`.
//!
//! Phase A is a static render: `update()` drives the shared
//! `run_raw_update` over the embedded `Buffer` using the Acrobat-style
//! render closure, and `getRustStorageBuffer()` exposes the `Buffer` for
//! the vbufRemote reads. There is no change sink yet (Phase B), so only
//! the initial render runs; the buffer is not re-rendered on DOM changes.

use core::ffi::c_void;

use windows::core::{w, Interface, BSTR, VARIANT};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{
    IAccessible, ObjectFromLresult,
};
use windows::Win32::UI::WindowsAndMessaging::{
    RegisterWindowMessageW, SendMessageW,
};

use nvda_vbuf::backend::run_raw_update;
use nvda_vbuf::storage::{Buffer, NodeKey};
use nvda_vbuf::{VbufBackend, VbufBuffer};

use crate::fill_vbuf::{fill_vbuf, get_frame_body, query_service, FillVBufCtx};
use crate::interfaces::{
    IHTMLDocument2, IHTMLDocument3, IHTMLDOMNode, IHTMLElement, IHTMLElement2,
};

/// Per-instance state owned by Rust, exposed to the C++ class as `void*`.
/// Phase A holds only the live storage tree.
pub struct MshtmlBackendState {
    pub buffer: Buffer,
}

impl MshtmlBackendState {
    fn new() -> Self {
        Self {
            buffer: Buffer::new(),
        }
    }
}

/// `WM_HTML_GETOBJECT`, registered on demand (RegisterWindowMessage
/// returns the same atom for the same string). Port of
/// `getHTMLWindowMessage`.
unsafe fn html_window_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("WM_HTML_GETOBJECT")) }
}

/// Resolve `(docHandle, id)` to the root `IHTMLDOMNode` to render. Port
/// of the initial-render half of C++ `render`: `WM_HTML_GETOBJECT` ->
/// `ObjectFromLresult(IHTMLDocument3)` -> `LocateHTMLElementInDocument` ->
/// `IHTMLDOMNode`. Phase A always locates by `ms__id<ID>` (no stored
/// pointer reuse); MSHTML's `getElementById("ms__id<N>")` resolves the
/// element whose unique number is `N`.
///
/// # Safety
///
/// COM apartment must be initialised; `doc_handle` is reinterpreted as an
/// `HWND`.
unsafe fn resolve_root_dom_node(
    doc_handle: i32,
    id: i32,
) -> Option<IHTMLDOMNode> {
    let hwnd = HWND(doc_handle as isize as *mut c_void);
    let msg = unsafe { html_window_message() };
    let res = unsafe { SendMessageW(hwnd, msg, WPARAM(0), LPARAM(0)) };
    if res.0 == 0 {
        return None;
    }
    let mut ppv: *mut c_void = core::ptr::null_mut();
    unsafe {
        ObjectFromLresult(res, &IHTMLDocument3::IID, WPARAM(0), &mut ppv)
    }
    .ok()?;
    if ppv.is_null() {
        return None;
    }
    let doc3 = unsafe { IHTMLDocument3::from_raw(ppv) };
    let id_str = format!("ms__id{id}");
    let element =
        unsafe { locate_html_element_in_document(&doc3, &id_str) }?;
    element.cast::<IHTMLDOMNode>().ok()
}

/// Port of C++ `LocateHTMLElementInDocument`: try `getElementById(id)` in
/// this document, else recurse into every FRAME/IFRAME subdocument.
///
/// # Safety
///
/// `doc3` must be a live `IHTMLDocument3`.
unsafe fn locate_html_element_in_document(
    doc3: &IHTMLDocument3,
    id: &str,
) -> Option<IHTMLElement> {
    // First try getting the element directly from this document.
    let id_bstr = BSTR::from(id);
    if let Ok(el) = unsafe { doc3.get_element_by_id(&id_bstr) } {
        return Some(el);
    }

    // Not here: search subdocuments. A FRAMESET body means FRAME children,
    // a plain body means IFRAME children.
    let doc2: IHTMLDocument2 = doc3.cast().ok()?;
    let body = unsafe { doc2.get_body() }.ok()?;
    let embedding_tag =
        match unsafe { body.get_tag_name() } {
            Ok(t) if t.as_wide() == weq("FRAMESET") => "FRAME",
            _ => "IFRAME",
        };
    let el2: IHTMLElement2 = body.cast().ok()?;
    let collection =
        unsafe { el2.get_elements_by_tag_name(&BSTR::from(embedding_tag)) }
            .ok()?;
    let num = unsafe { collection.get_length() }.unwrap_or(0);
    for index in 0..num {
        let disp = match unsafe {
            collection.item(&VARIANT::from(index), &VARIANT::from(0i32))
        } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let pacc: IAccessible =
            match unsafe { query_service(&disp, &IAccessible::IID) } {
                Some(a) => a,
                None => continue,
            };
        let sub_body_dom = match unsafe { get_frame_body(&pacc) } {
            Some(n) => n,
            None => continue,
        };
        let sub_body_el: IHTMLElement = match sub_body_dom.cast() {
            Ok(e) => e,
            Err(_) => continue,
        };
        let doc_disp = match unsafe { sub_body_el.get_document() } {
            Ok(d) => d,
            Err(_) => return None,
        };
        let sub_doc3: IHTMLDocument3 = match doc_disp.cast() {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Some(found) =
            unsafe { locate_html_element_in_document(&sub_doc3, id) }
        {
            return Some(found);
        }
    }
    None
}

/// UTF-16 of a literal, for tag-name comparison.
fn weq(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

// --- C-ABI entry points ---------------------------------------------------

/// Allocate a [`MshtmlBackendState`]. Pair with
/// [`mshtml_backend_destroy`].
#[no_mangle]
pub extern "C" fn mshtml_backend_create() -> *mut c_void {
    Box::into_raw(Box::new(MshtmlBackendState::new())) as *mut c_void
}

/// Free a [`MshtmlBackendState`]. Accepts `NULL` as a no-op.
///
/// # Safety
///
/// `state` must be `NULL` or a pointer from [`mshtml_backend_create`] not
/// yet destroyed.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state as *mut MshtmlBackendState) });
}

/// Address of this backend's embedded `Buffer`, for
/// `MshtmlVBufBackend_t::getRustStorageBuffer`.
///
/// # Safety
///
/// `state` must be a valid `MshtmlBackendState*`; the returned pointer is
/// valid while the state lives and must be dereferenced only under the
/// backend lock.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_get_buffer(
    state: *mut c_void,
) -> *mut Buffer {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let state = unsafe { &mut *(state as *mut MshtmlBackendState) };
    &mut state.buffer as *mut Buffer
}

/// Empty this backend's `Buffer` (render-thread terminate / new document).
///
/// # Safety
///
/// `state` must be a valid `MshtmlBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_clear_buffer(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut MshtmlBackendState) };
    state.buffer.clear();
}

/// Whether a control node `(doc_handle, id)` exists in this backend's
/// `Buffer`. Backs the C++ change sink's `getDeepestControlFieldNodeFor
/// HTMLElement` walk (it walks a changed element up its ancestors calling
/// this until it finds the deepest element that has a rendered node).
///
/// # Safety
///
/// `state` must be a valid `MshtmlBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_has_node(
    state: *mut c_void,
    doc_handle: i32,
    id: i32,
) -> bool {
    if state.is_null() {
        return false;
    }
    let state = unsafe { &*(state as *mut MshtmlBackendState) };
    state
        .buffer
        .get_control_field_node_with_identifier(doc_handle, id)
        .is_some()
}

/// Ancestors of `key` from the root down to `key` inclusive.
fn ancestors_root_first(buffer: &Buffer, key: NodeKey) -> Vec<NodeKey> {
    let mut v = vec![key];
    let mut cur = key;
    while let Some(p) = buffer.parent_of(cur) {
        v.push(p);
        cur = p;
    }
    v.reverse();
    v
}

/// Deepest common ancestor of two nodes (the last shared entry walking
/// from the root). Mirrors the ancestor-list intersection in the C++
/// `CHTMLChangeSink::Notify`.
fn deepest_common_ancestor(
    buffer: &Buffer,
    a: NodeKey,
    b: NodeKey,
) -> Option<NodeKey> {
    let aa = ancestors_root_first(buffer, a);
    let bb = ancestors_root_first(buffer, b);
    let mut common = None;
    for (x, y) in aa.iter().zip(bb.iter()) {
        if x == y {
            common = Some(*x);
        } else {
            break;
        }
    }
    common
}

/// Invalidate the subtree covering a dirty range `[begin_id, end_id]`
/// (either may be `0` meaning "no rendered node found"), then arm the
/// update timer. Port of `CHTMLChangeSink::Notify`'s node-selection +
/// `invalidateSubtree`/`requestUpdate` tail, operating on the Rust buffer:
/// if the two ends resolve to the same node (or only one resolves) that
/// node is invalidated; if both resolve to different nodes their deepest
/// common ancestor is.
///
/// # Safety
///
/// `state` must be a valid `MshtmlBackendState*`; `backend` a valid
/// `VBufBackend_t*`.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_invalidate_range(
    state: *mut c_void,
    backend: *mut c_void,
    doc_handle: i32,
    begin_id: i32,
    end_id: i32,
) {
    if state.is_null() || backend.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut MshtmlBackendState) };
    let backend_h = VbufBackend(backend);
    let lookup = |id: i32| -> Option<NodeKey> {
        if id == 0 {
            None
        } else {
            state
                .buffer
                .get_control_field_node_with_identifier(doc_handle, id)
        }
    };
    let begin = lookup(begin_id);
    let end = lookup(end_id);
    let invalid = match (begin, end) {
        (Some(b), Some(e)) if b == e => Some(b),
        (Some(b), None) => Some(b),
        (None, Some(e)) => Some(e),
        (Some(b), Some(e)) => deepest_common_ancestor(&state.buffer, b, e),
        (None, None) => None,
    };
    if let Some(k) = invalid {
        if state.buffer.invalidate_subtree(k) {
            unsafe { backend_h.request_update() };
        }
    }
}

/// Drain/render/merge orchestration over the embedded `Buffer`. Backs
/// `MshtmlVBufBackend_t::update()`. Returns `true` when the caller should
/// fire `vbufChangeNotify` (re-render branch), `false` on the initial
/// render.
///
/// # Safety
///
/// * `state` must be a valid `MshtmlBackendState*`.
/// * `backend` must be a valid `VBufBackend_t*`.
/// * Must run on the render thread with the backend lock held.
#[no_mangle]
pub unsafe extern "C" fn mshtml_backend_update(
    state: *mut c_void,
    backend: *mut c_void,
) -> bool {
    if state.is_null() || backend.is_null() {
        return false;
    }
    let state = unsafe { &mut *(state as *mut MshtmlBackendState) };
    let backend_h = VbufBackend(backend);
    let root_doc_handle = unsafe { backend_h.root_doc_handle() };
    let root_id = unsafe { backend_h.root_id() };

    let main_ptr: *mut Buffer = &mut state.buffer as *mut Buffer;
    unsafe {
        run_raw_update(
            main_ptr,
            root_doc_handle,
            root_id,
            |target, _main, doc_handle, id, _old_node| {
                let dom_node = match resolve_root_dom_node(doc_handle, id) {
                    Some(n) => n,
                    None => return false,
                };
                let ctx = FillVBufCtx {
                    doc_handle,
                    root_id,
                };
                fill_vbuf(VbufBuffer(target), None, None, &dom_node, &ctx);
                true
            },
        )
    }
}
