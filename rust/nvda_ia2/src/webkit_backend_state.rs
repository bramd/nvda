//! Per-instance Rust state for `WebKitVBufBackend_t`.
//!
//! WebKit's live tree lives in an embedded `storage::Buffer`, exactly like
//! gecko's (see [`crate::gecko_backend_state`]) but with far less state:
//! no cached toolkit name, no cached root accessible, no defunct-document
//! check. The C++ backend holds a `void* rustState` allocated by
//! [`nvda_ia2_webkit_backend_create`] and freed by
//! [`nvda_ia2_webkit_backend_destroy`].

use core::ffi::c_void;

use crate::from_identifier::from_identifier;
use crate::interfaces::IAccessible2;
use crate::webkit_fill_vbuf::fill_vbuf;
use nvda_vbuf::backend::run_raw_update;
use nvda_vbuf::storage::Buffer;
use nvda_vbuf::VbufBackend;
use nvda_vbuf::VbufBuffer;

// MSAA event IDs (oleacc.h) the WebKit hook reacts to.
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800a;
const EVENT_OBJECT_VALUECHANGE: u32 = 0x800e;

/// Per-instance state owned by the Rust side and exposed through a `void*`
/// to the C++ class. Just the live WebKit storage tree.
pub struct WebKitBackendState {
    pub buffer: Buffer,
}

impl WebKitBackendState {
    pub fn new() -> Self {
        Self {
            buffer: Buffer::new(),
        }
    }
}

impl Default for WebKitBackendState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a `(docHandle, id)` pair to an `IAccessible2`, mirroring the
/// WebKit-specific `IAccessible2FromIdentifier`
/// (`webKit.cpp:28`): identical to gecko's [`from_identifier`] except
/// WebKit stores positive unique IDs but must be queried with the sign
/// flipped.
///
/// # Safety
///
/// Same obligations as [`from_identifier`]: the COM apartment must be
/// initialised and `doc_handle` is reinterpreted as an HWND.
unsafe fn webkit_from_identifier(
    doc_handle: i32,
    id: i32,
) -> Option<IAccessible2> {
    unsafe { from_identifier(doc_handle, -id) }
}

/// Allocate a `WebKitBackendState` and return a raw `void*` for the C++
/// class to hold. Pair with [`nvda_ia2_webkit_backend_destroy`].
#[no_mangle]
pub extern "C" fn nvda_ia2_webkit_backend_create() -> *mut c_void {
    Box::into_raw(Box::new(WebKitBackendState::new())) as *mut c_void
}

/// Free a `WebKitBackendState` from [`nvda_ia2_webkit_backend_create`].
/// Accepts `NULL` as a no-op.
///
/// # Safety
///
/// `state` must be `NULL` or a pointer from
/// `nvda_ia2_webkit_backend_create` not yet destroyed.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_webkit_backend_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state as *mut WebKitBackendState) });
}

/// Address of this backend's embedded `storage::Buffer`. Backs
/// `WebKitVBufBackend_t::getRustStorageBuffer` and, transitively, the
/// vbufRemote read RPCs.
///
/// # Safety
///
/// `state` must be a valid `WebKitBackendState*`. The returned pointer is
/// valid for the state's lifetime and must only be dereferenced under the
/// backend lock.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_webkit_backend_get_buffer(
    state: *mut c_void,
) -> *mut Buffer {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let state = unsafe { &mut *(state as *mut WebKitBackendState) };
    &mut state.buffer as *mut Buffer
}

/// Empty this backend's embedded `storage::Buffer` (terminate path).
///
/// # Safety
///
/// `state` must be a valid `WebKitBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_webkit_backend_clear_buffer(
    state: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut WebKitBackendState) };
    state.buffer.clear();
}

/// Drain/render/merge orchestration over the embedded `state.buffer`.
/// Backs `WebKitVBufBackend_t::update()`. The storage-side control flow
/// lives in the shared [`run_raw_update`]; this is the WebKit adapter,
/// supplying the render closure (resolve an `IAccessible2` for a
/// `(docHandle, ID)` and run [`fill_vbuf`]). WebKit ignores both the
/// `main` buffer and `old_node` — it renders every invalidated subtree
/// fresh, as the C++ backend did.
///
/// Returns the base `update()`'s notify condition: `true` after a
/// re-render (caller fires `vbufChangeNotify`), `false` after the initial
/// render. The Win32 notify itself is left to the C++ caller.
///
/// # Safety
///
/// * `state` must be a valid `WebKitBackendState*`; `backend` a valid
///   `VBufBackend_t*`.
/// * Must run on the render thread with the backend lock held.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_webkit_backend_update(
    state: *mut c_void,
    backend: *mut c_void,
) -> bool {
    if state.is_null() || backend.is_null() {
        return false;
    }
    let state = unsafe { &mut *(state as *mut WebKitBackendState) };
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
                let acc = match webkit_from_identifier(doc_handle, id) {
                    Some(a) => a,
                    None => return false,
                };
                let _ = fill_vbuf(
                    doc_handle,
                    &acc,
                    VbufBuffer(target),
                    None,
                    None,
                );
                true
            },
        )
    }
}

/// Outer-loop event filter for the WinEvent hook: `true` for the three
/// events WebKit re-renders on (`webKit.cpp:178`). The per-backend hwnd
/// match + node lookup happen in the C++ hook /
/// [`nvda_ia2_webkit_backend_dispatch_win_event`].
#[no_mangle]
pub extern "C" fn nvda_ia2_webkit_backend_win_event_is_relevant(
    event_id: u32,
) -> bool {
    matches!(
        event_id,
        EVENT_OBJECT_VALUECHANGE
            | EVENT_OBJECT_STATECHANGE
            | EVENT_OBJECT_REORDER
    )
}

/// Per-backend dispatch for the WinEvent hook (`webKit.cpp:197-203`).
/// Looks up the affected node by `(doc_handle, -child_id)` — WebKit fires
/// events with the sign of the unique ID flipped — invalidates its subtree
/// in the Rust buffer, and arms the render-thread timer.
///
/// # Safety
///
/// `state` must be a valid `WebKitBackendState*`; `backend` a valid
/// `VBufBackend_t*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_webkit_backend_dispatch_win_event(
    state: *mut c_void,
    backend: *mut c_void,
    doc_handle: i32,
    child_id: i32,
) {
    if state.is_null() || backend.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut WebKitBackendState) };
    let backend_h = VbufBackend(backend);
    // WebKit fires events with negated unique IDs; the buffer stores the
    // positive value.
    let key = match state
        .buffer
        .get_control_field_node_with_identifier(doc_handle, -child_id)
    {
        Some(k) => k,
        None => return,
    };
    if state.buffer.invalidate_subtree(key) {
        unsafe { backend_h.request_update() };
    }
}
