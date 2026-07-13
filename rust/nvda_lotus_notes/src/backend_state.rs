//! Per-instance Rust state for `lotusNotesRichTextVBufBackend_t`.
//!
//! Homes the live tree in an embedded `storage::Buffer` and exposes the
//! C-ABI entry points the thin C++ adapter calls. Structurally identical
//! to the webKit adapter (see `nvda_ia2::webkit_backend_state`), minus the
//! id-sign-flip: lotusNotesRichText uses MSAA child IDs directly, both in
//! the buffer and in the WinEvents it fires.

use core::ffi::c_void;

use crate::fill_vbuf::{
    render_control_content, render_root, resolve_client_iaccessible,
};
use nvda_vbuf::backend::run_raw_update;
use nvda_vbuf::storage::Buffer;
use nvda_vbuf::VbufBackend;
use nvda_vbuf::VbufBuffer;

// MSAA event IDs (oleacc.h) the Lotus Notes hook reacts to.
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800a;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800c;
const EVENT_OBJECT_VALUECHANGE: u32 = 0x800e;

/// Per-instance state owned by the Rust side and exposed through a `void*`
/// to the C++ class. Just the live Lotus Notes storage tree.
pub struct LotusNotesBackendState {
    pub buffer: Buffer,
}

impl LotusNotesBackendState {
    pub fn new() -> Self {
        Self {
            buffer: Buffer::new(),
        }
    }
}

impl Default for LotusNotesBackendState {
    fn default() -> Self {
        Self::new()
    }
}

/// Allocate a `LotusNotesBackendState`; pair with
/// [`nvda_lotus_notes_backend_destroy`].
#[no_mangle]
pub extern "C" fn nvda_lotus_notes_backend_create() -> *mut c_void {
    Box::into_raw(Box::new(LotusNotesBackendState::new())) as *mut c_void
}

/// Free a `LotusNotesBackendState` from
/// [`nvda_lotus_notes_backend_create`]. Accepts `NULL` as a no-op.
///
/// # Safety
///
/// `state` must be `NULL` or a pointer from
/// `nvda_lotus_notes_backend_create` not yet destroyed.
#[no_mangle]
pub unsafe extern "C" fn nvda_lotus_notes_backend_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state as *mut LotusNotesBackendState) });
}

/// Address of this backend's embedded `storage::Buffer`. Backs
/// `getRustStorageBuffer()` and, transitively, the vbufRemote read RPCs.
///
/// # Safety
///
/// `state` must be a valid `LotusNotesBackendState*`; the pointer is valid
/// for the state's lifetime and only usable under the backend lock.
#[no_mangle]
pub unsafe extern "C" fn nvda_lotus_notes_backend_get_buffer(
    state: *mut c_void,
) -> *mut Buffer {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let state = unsafe { &mut *(state as *mut LotusNotesBackendState) };
    &mut state.buffer as *mut Buffer
}

/// Empty this backend's embedded `storage::Buffer` (terminate path).
///
/// # Safety
///
/// `state` must be a valid `LotusNotesBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_lotus_notes_backend_clear_buffer(
    state: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut LotusNotesBackendState) };
    state.buffer.clear();
}

/// Drain/render/merge orchestration over the embedded `state.buffer`.
/// Backs `lotusNotesRichTextVBufBackend_t::update()`. The render closure
/// resolves the document's client `IAccessible` and reproduces C++
/// `render`'s two modes: `id == 0` rebuilds the whole tree (the synthetic
/// client root + its children); a nonzero `id` re-renders that single
/// child's subtree (into `run_raw_update`'s temp buffer, which is then
/// grafted back).
///
/// Returns the base `update()`'s notify condition: `true` after a
/// re-render, `false` after the initial render. The Win32 notify itself is
/// left to the C++ caller.
///
/// # Safety
///
/// * `state` must be a valid `LotusNotesBackendState*`; `backend` a valid
///   `VBufBackend_t*`.
/// * Must run on the render thread with the backend lock held.
#[no_mangle]
pub unsafe extern "C" fn nvda_lotus_notes_backend_update(
    state: *mut c_void,
    backend: *mut c_void,
) -> bool {
    if state.is_null() || backend.is_null() {
        return false;
    }
    let state = unsafe { &mut *(state as *mut LotusNotesBackendState) };
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
                let pacc = match resolve_client_iaccessible(doc_handle) {
                    Some(a) => a,
                    None => return false,
                };
                let buffer = VbufBuffer(target);
                if id == 0 {
                    render_root(doc_handle, &pacc, buffer);
                } else {
                    let _ = render_control_content(
                        doc_handle, &pacc, id, buffer, None, None,
                    );
                }
                true
            },
        )
    }
}

/// Outer-loop event filter for the WinEvent hook: `true` for the four
/// events Lotus Notes re-renders on (`lotusNotesRichText.cpp:38`).
#[no_mangle]
pub extern "C" fn nvda_lotus_notes_backend_win_event_is_relevant(
    event_id: u32,
) -> bool {
    matches!(
        event_id,
        EVENT_OBJECT_REORDER
            | EVENT_OBJECT_NAMECHANGE
            | EVENT_OBJECT_VALUECHANGE
            | EVENT_OBJECT_STATECHANGE
    )
}

/// Per-backend dispatch for the WinEvent hook
/// (`lotusNotesRichText.cpp:60-64`). Looks up the affected node by
/// `(doc_handle, child_id)` — Lotus Notes fires unsigned MSAA child IDs,
/// no sign flip — invalidates its subtree, and arms the render-thread
/// timer.
///
/// # Safety
///
/// `state` must be a valid `LotusNotesBackendState*`; `backend` a valid
/// `VBufBackend_t*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_lotus_notes_backend_dispatch_win_event(
    state: *mut c_void,
    backend: *mut c_void,
    doc_handle: i32,
    child_id: i32,
) {
    if state.is_null() || backend.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut LotusNotesBackendState) };
    let backend_h = VbufBackend(backend);
    let key = match state
        .buffer
        .get_control_field_node_with_identifier(doc_handle, child_id)
    {
        Some(k) => k,
        None => return,
    };
    if state.buffer.invalidate_subtree(key) {
        unsafe { backend_h.request_update() };
    }
}
