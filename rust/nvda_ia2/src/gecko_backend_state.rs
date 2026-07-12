//! Per-instance Rust state for `GeckoVBufBackend_t`.
//!
//! Each `GeckoVBufBackend_t` instance carries a `void* rustState`
//! pointer that is allocated by [`nvda_ia2_gecko_backend_create`] in
//! the C++ constructor and freed by [`nvda_ia2_gecko_backend_destroy`]
//! in the destructor. Per-Phase-5 work, fields move from C++ class
//! members into this struct as their consumers migrate to Rust.
//!
//! Currently holds:
//!
//! * `toolkit_name` — populated by `versionSpecificInit`; consumed by
//!   `fillVBuf`'s `is_chrome` check.

use core::ffi::c_void;

use windows::core::Interface;

use crate::fill_vbuf::{fill_vbuf, FillVBufCtx};
use crate::from_identifier::from_identifier;
use crate::interfaces::IAccessible2;
use crate::toolkit_name::get_toolkit_name_native;
use nvda_vbuf::storage::Buffer;
use nvda_vbuf::storage::{FieldNodeKind, NodeKey};
use nvda_vbuf::VbufBackend;
use nvda_vbuf::VbufBuffer;

// MSAA event IDs (oleacc.h).
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_OBJECT_FOCUS: u32 = 0x8005;
const EVENT_OBJECT_SELECTIONADD: u32 = 0x8007;
const EVENT_OBJECT_SELECTIONREMOVE: u32 = 0x8008;
const EVENT_OBJECT_SELECTIONWITHIN: u32 = 0x8009;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800a;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800c;
const EVENT_OBJECT_DESCRIPTIONCHANGE: u32 = 0x800d;
const EVENT_OBJECT_VALUECHANGE: u32 = 0x800e;
const EVENT_SYSTEM_ALERT: u32 = 0x0002;

// IA2 event IDs (build/<arch>/ia2.h, derived from
// IA2_EVENT_ACTION_CHANGED = 0x101).
const IA2_EVENT_DOCUMENT_LOAD_COMPLETE: u32 = 0x105;
const IA2_EVENT_OBJECT_ATTRIBUTE_CHANGED: u32 = 0x110;
const IA2_EVENT_TEXT_ATTRIBUTE_CHANGED: u32 = 0x11a;
const IA2_EVENT_TEXT_INSERTED: u32 = 0x11e;
const IA2_EVENT_TEXT_REMOVED: u32 = 0x11f;
const IA2_EVENT_TEXT_UPDATED: u32 = 0x120;

/// `OBJID_CLIENT` per oleacc.h (-4 as a signed i32).
const OBJID_CLIENT: i32 = -4;

/// Per-backend dispatch outcomes for the WinEvent hook.
#[repr(i32)]
pub enum WinEventOutcome {
    /// Continue iterating to the next running backend.
    Continue = 0,
    /// Stop the whole hook function (used for state-change on the
    /// root document, where re-render would cause a busy-state loop).
    StopAll = 1,
}

/// Per-instance state owned by the Rust side and exposed through
/// `void*` to the C++ class.
pub struct GeckoBackendState {
    pub toolkit_name: Vec<u16>,
    /// Cached `IAccessible2` for the root document. Set up by
    /// `renderThread_initialize`, released by
    /// `renderThread_terminate`. Holds an AddRef'd reference; on
    /// drop the [`Drop`] impl deliberately *leaks* a non-`None`
    /// value (mirrors `CComPtr::Detach()` in the C++ destructor)
    /// because the Rust drop path may run on a thread different
    /// from the one that created the COM pointer.
    pub root_doc_acc: Option<IAccessible2>,
    /// The live gecko storage tree (Phase 6e). Rendered into and read
    /// out of by the gecko backend; the C++ `VBufStorage_buffer_t` is no
    /// longer used for gecko storage. Embedding it here ties its
    /// lifetime to the per-backend Rust state (created/destroyed with
    /// `GeckoBackendState`).
    pub buffer: Buffer,
}

impl GeckoBackendState {
    pub fn new() -> Self {
        Self {
            toolkit_name: Vec::new(),
            root_doc_acc: None,
            buffer: Buffer::new(),
        }
    }

    /// `true` when the cached toolkit name is `"Chrome"`. Used by
    /// `fillVBuf` for the IA2 `relationTargetsOfType` workaround.
    pub fn is_chrome(&self) -> bool {
        self.toolkit_name == "Chrome".encode_utf16().collect::<Vec<u16>>()
    }

    /// Populate the cached toolkit name from `acc`. Mirrors
    /// `versionSpecificInit`; shared by that entry point and the
    /// root-call branch of `render`.
    fn init_toolkit_name(&mut self, acc: &IAccessible2) {
        self.toolkit_name = get_toolkit_name_native(acc);
    }
}

impl Default for GeckoBackendState {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GeckoBackendState {
    fn drop(&mut self) {
        // Mirror the C++ destructor's `nhAssert(!rootDocAcc); if
        // (this->rootDocAcc) this->rootDocAcc.Detach();` -- if the
        // root accessible was never released by terminate, leak the
        // AddRef. Releasing a COM pointer from the wrong thread can
        // crash; see https://issues.chromium.org/issues/41487612.
        if let Some(acc) = self.root_doc_acc.take() {
            core::mem::forget(acc);
        }
    }
}

/// Allocate a `GeckoBackendState` and return a raw `void*` for the
/// C++ class to hold. Caller must pair with
/// [`nvda_ia2_gecko_backend_destroy`].
#[no_mangle]
pub extern "C" fn nvda_ia2_gecko_backend_create() -> *mut c_void {
    Box::into_raw(Box::new(GeckoBackendState::new())) as *mut c_void
}

/// Free a `GeckoBackendState` previously returned by
/// [`nvda_ia2_gecko_backend_create`]. Accepts `NULL` as a no-op.
///
/// # Safety
///
/// `state` must be either `NULL` or a pointer previously returned by
/// `nvda_ia2_gecko_backend_create` and not yet destroyed.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_destroy(
    state: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state as *mut GeckoBackendState) });
}

/// Address of this backend's embedded Rust `storage::Buffer`. Backs
/// `GeckoVBufBackend_t::getRustStorageBuffer` (routed through
/// `nvda_ia2_gecko_backend_get_buffer`) and, transitively, the
/// vbufRemote read RPCs and the update orchestration.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*` from
/// [`nvda_ia2_gecko_backend_create`]. The returned pointer is valid
/// for as long as the `GeckoBackendState` lives and must only be
/// dereferenced under the backend lock.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_get_buffer(
    state: *mut c_void,
) -> *mut Buffer {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    &mut state.buffer as *mut Buffer
}

/// Clear this backend's embedded Rust `storage::Buffer`. Called from
/// the gecko terminate / defunct-document paths (Phase 6e, Decision 5)
/// so the Rust tree is emptied alongside the C++ storage.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_clear_buffer(
    state: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    state.buffer.clear();
}

/// Read accessor: `1` when the cached toolkit name is `"Chrome"`,
/// `0` otherwise. Used by the C++ `fillVBuf` shim.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*` from
/// [`nvda_ia2_gecko_backend_create`].
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_is_chrome(
    state: *mut c_void,
) -> i32 {
    if state.is_null() {
        return 0;
    }
    let state = unsafe { &*(state as *const GeckoBackendState) };
    if state.is_chrome() {
        1
    } else {
        0
    }
}

/// Populate the cached toolkit name on this backend. Mirrors
/// `versionSpecificInit`: walks `IAccessible2` → `IServiceProvider`
/// → `IAccessibleApplication::get_toolkitName` and stores the
/// resulting string in the per-instance state.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`; `pacc` must be a
/// valid `IAccessible2*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_version_specific_init(
    state: *mut c_void,
    pacc: *mut c_void,
) {
    if state.is_null() || pacc.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    let acc: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc) {
        Some(a) => a,
        None => return,
    };
    state.init_toolkit_name(acc);
}

/// Drain/render/merge orchestration over the embedded Rust
/// `state.buffer`. Backs `GeckoVBufBackend_t::update()` once gecko
/// overrides `update()` at the flip (Phase 6e, Decision 3). Mirrors
/// `nvda_vbuf::backend::update` (and the C++ `VBufBackend_t::update`)
/// but in raw-pointer form so the cross-buffer reuse lookups
/// (`fill_vbuf` querying `state.buffer` while writing a temp buffer)
/// don't require a competing `&mut state.buffer` borrow across the
/// render call.
///
/// The `nvdaControllerInternal_vbufChangeNotify` call is deliberately
/// left to the C++ caller (it's Win32 integration, not storage), exactly
/// as `nvda_vbuf::backend::update`'s doc-comment notes. The **return
/// value** carries the base `VBufBackend_t::update`'s notify condition
/// back to that caller: `VBufBackend_t::update` fires the change-notify
/// only on the re-render (`hasContent`) branch, never on the initial
/// render. This function therefore returns `true` when it took the
/// re-render branch (the caller should notify) and `false` for the
/// initial render (the caller must not notify), reproducing that
/// contract exactly rather than notifying unconditionally.
///
/// # Safety
///
/// * `state` must be a valid `GeckoBackendState*`.
/// * `backend` must be a valid `VBufBackend_t*`.
/// * Must run on the render thread with the backend lock held (the C++
///   `update()` override acquires it), so no other thread touches
///   `state.buffer` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_update(
    state: *mut c_void,
    backend: *mut c_void,
) -> bool {
    if state.is_null() || backend.is_null() {
        return false;
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    let backend_h = VbufBackend(backend);
    let root_doc_handle = unsafe { backend_h.root_doc_handle() };
    let root_id = unsafe { backend_h.root_id() };

    // Initial render: an empty buffer is rendered straight into
    // `state.buffer` (it has no content to displace). Mirrors the base
    // `update()`'s `else` branch, which does not fire vbufChangeNotify —
    // so return `false`.
    if !state.buffer.has_content() {
        let acc = match unsafe { from_identifier(root_doc_handle, root_id) } {
            Some(a) => a,
            None => return false,
        };
        state.init_toolkit_name(&acc);
        let is_chrome = state.is_chrome();
        // Take the raw pointer only now, right before the render, so no
        // `&mut state` reborrow (init_toolkit_name above) invalidates it.
        let main_ptr: *mut Buffer = &mut state.buffer as *mut Buffer;
        let ctx = FillVBufCtx {
            backend: backend_h,
            main: VbufBuffer(main_ptr),
            root_id,
            is_chrome,
        };
        let _ = unsafe {
            fill_vbuf(
                &acc,
                VbufBuffer(main_ptr),
                None,
                None,
                None,
                0,
                None,
                false,
                &ctx,
            )
        };
        return false;
    }

    // Re-render path: drain pending invalidations into the working
    // list, re-render each into its own temp buffer (reuse queries
    // `state.buffer` via `ctx.main`), then atomically merge.
    let is_chrome = state.is_chrome();
    // From here on `state.buffer` is reached *only* through `main_ptr`
    // so a single raw-pointer provenance covers every access and no
    // stacked `&mut state.buffer` borrow crosses a `fill_vbuf` call.
    let main_ptr: *mut Buffer = &mut state.buffer as *mut Buffer;
    let working_keys = unsafe { (*main_ptr).take_pending_into_working() };
    let mut map: Vec<(NodeKey, Buffer)> = Vec::new();
    for key in working_keys {
        // Identifier of the invalidated subtree root, looked up from
        // main. Skip stale keys (a prior cascade may have removed one).
        let identifier =
            match unsafe { (*main_ptr).identifier_of_control_field_node(key) } {
                Some(i) => i,
                None => continue,
            };
        let acc = match unsafe {
            from_identifier(identifier.doc_handle, identifier.id)
        } {
            Some(a) => a,
            None => continue,
        };
        let mut temp = Buffer::new();
        let temp_ptr: *mut Buffer = &mut temp as *mut Buffer;
        let ctx = FillVBufCtx {
            backend: backend_h,
            main: VbufBuffer(main_ptr),
            root_id,
            is_chrome,
        };
        let _ = unsafe {
            fill_vbuf(
                &acc,
                VbufBuffer(temp_ptr),
                None,
                None,
                None,
                0,
                None,
                false,
                &ctx,
            )
        };
        // `temp` is moved into `map` (its arena lives on the heap, so
        // the move is address-stable for the stored nodes); `temp_ptr`
        // is not used past this point.
        map.push((key, temp));
    }
    unsafe { (*main_ptr).clear_working() };
    unsafe { (*main_ptr).replace_subtrees(map) };
    // Re-render branch taken: mirror the base `update()`'s `hasContent`
    // arm, which fires vbufChangeNotify. Tell the C++ caller to notify.
    true
}

/// C-callable replacement for the body of
/// `GeckoVBufBackend_t::renderThread_initialize` past the parent-
/// class call: resolves the root `(doc_handle, id)` to an
/// `IAccessible2` and stashes it on the state. Subsequent calls
/// replace the cached value (which is unusual but matches the C++
/// `CComPtr::operator=` behavior).
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`. Caller must hold
/// the render-thread invariants vbufBase requires.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_render_thread_initialize(
    state: *mut c_void,
    doc_handle: i32,
    id: i32,
) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    state.root_doc_acc = unsafe { from_identifier(doc_handle, id) };
}

/// C-callable replacement for the body of
/// `GeckoVBufBackend_t::renderThread_terminate` past the parent-
/// class call: releases the cached root `IAccessible2`.
///
/// Must be called on the same thread that invoked
/// `nvda_ia2_gecko_backend_render_thread_initialize` -- COM
/// `Release` is thread-affine for the kinds of object Gecko / Chrome
/// expose here.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_render_thread_terminate(
    state: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    // Take + drop releases on this thread.
    let _ = state.root_doc_acc.take();
}

/// State-aware `isRootDocAlive`: reads `root_doc_acc` from the
/// per-instance state, runs the pending-update short-circuit + the
/// `IA2_STATE_DEFUNCT` check, and clears `root_doc_acc` when the
/// document is dead (matching the C++ original which set
/// `this->rootDocAcc = nullptr`).
///
/// Returns `1` (alive) or `0` (dead).
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`. `backend` must be a
/// valid `VBufBackend_t*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_is_root_doc_alive(
    state: *mut c_void,
    backend: *mut c_void,
) -> i32 {
    if state.is_null() || backend.is_null() {
        return 0;
    }
    // Pending update -> short-circuit alive. The pending-invalid list
    // lives on `state.buffer` (gecko invalidates the Rust storage
    // directly), so the C++ backend's list is always empty for gecko and
    // must not be consulted (Decision 5).
    let has_pending = !unsafe {
        (*(state as *mut GeckoBackendState))
            .buffer
            .pending_invalid_subtrees_empty()
    };
    if has_pending {
        return 1;
    }

    let state = unsafe { &mut *(state as *mut GeckoBackendState) };
    let alive = match state.root_doc_acc.as_ref() {
        Some(acc) => match unsafe { acc.get_states() } {
            Ok(s) => (s & IA2_STATE_DEFUNCT) == 0,
            Err(_) => false,
        },
        None => false,
    };
    if !alive {
        // Take + drop releases on the calling thread.
        let _ = state.root_doc_acc.take();
    }
    if alive {
        1
    } else {
        0
    }
}

/// `IA2_STATE_DEFUNCT` from `AccessibleStates.idl:93`.
const IA2_STATE_DEFUNCT: i32 = 0x4;

/// Outer-loop event filter for the WinEvent hook. Returns `true`
/// when the event is one of the IDs gecko_ia2 cares about, the
/// objectID is `OBJID_CLIENT`, and the childID is "self" or a
/// negative IA2 unique ID. Mirrors the early returns at the top of
/// the C++ `renderThread_winEventProcHook`.
#[no_mangle]
pub extern "C" fn nvda_ia2_gecko_backend_win_event_is_relevant(
    event_id: u32,
    hwnd: *mut c_void,
    object_id: i32,
    child_id: i32,
) -> bool {
    let allowed = matches!(
        event_id,
        EVENT_OBJECT_FOCUS
            | IA2_EVENT_DOCUMENT_LOAD_COMPLETE
            | EVENT_SYSTEM_ALERT
            | IA2_EVENT_TEXT_UPDATED
            | IA2_EVENT_TEXT_INSERTED
            | IA2_EVENT_TEXT_REMOVED
            | EVENT_OBJECT_REORDER
            | EVENT_OBJECT_NAMECHANGE
            | EVENT_OBJECT_VALUECHANGE
            | EVENT_OBJECT_DESCRIPTIONCHANGE
            | EVENT_OBJECT_STATECHANGE
            | EVENT_OBJECT_SELECTIONADD
            | EVENT_OBJECT_SELECTIONREMOVE
            | EVENT_OBJECT_SELECTIONWITHIN
            | IA2_EVENT_OBJECT_ATTRIBUTE_CHANGED
            | IA2_EVENT_TEXT_ATTRIBUTE_CHANGED
            | EVENT_OBJECT_HIDE
    );
    if !allowed {
        return false;
    }
    if child_id >= 0 || object_id != OBJID_CLIENT {
        return false;
    }
    if hwnd.is_null() {
        return false;
    }
    true
}

/// Per-backend dispatch for the WinEvent hook. Mirrors the body of
/// the C++ inner loop at gecko_ia2.cpp:177-228 (pre-flip).
///
/// `state` is this backend's `GeckoBackendState`; `backend` is the
/// `VBufBackend_t*` for vbuf-base operations (forceUpdate, etc.).
/// `doc_handle` and `id` are derived from the WinEvent's `hwnd` and
/// `childID`.
///
/// Returns one of [`WinEventOutcome`] values. The C++ caller treats
/// `StopAll` as "exit the entire hook function" -- used when a
/// state-change event fires on the root document.
///
/// # Safety
///
/// `state` must be a valid `GeckoBackendState*`; `backend` must be
/// a valid `VBufBackend_t*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_dispatch_win_event(
    state: *mut c_void,
    backend: *mut c_void,
    event_id: u32,
    doc_handle: i32,
    id: i32,
) -> i32 {
    if state.is_null() || backend.is_null() {
        return WinEventOutcome::Continue as i32;
    }
    let backend_h = VbufBackend(backend);

    // For focus, document-load-complete, and alert: force any
    // already-pending updates to apply now. These events trigger an
    // immediate flush rather than scheduling a re-render.
    if matches!(
        event_id,
        EVENT_OBJECT_FOCUS
            | IA2_EVENT_DOCUMENT_LOAD_COMPLETE
            | EVENT_SYSTEM_ALERT
    ) {
        unsafe { backend_h.force_update() };
        return WinEventOutcome::Continue as i32;
    }

    // Ignore state-change events on the root document; the
    // document-busy bit toggling causes spurious re-renders.
    let backend_doc_handle = unsafe { backend_h.root_doc_handle() };
    let backend_root_id = unsafe { backend_h.root_id() };
    if event_id == EVENT_OBJECT_STATECHANGE
        && doc_handle == backend_doc_handle
        && id == backend_root_id
    {
        return WinEventOutcome::StopAll as i32;
    }

    // Storage tail (Decision 5): operate on `state.buffer` directly
    // (Rust slotmap keys) and arm the render-thread timer through the
    // c_shim `requestUpdate` forwarder.
    let state_ptr = state as *mut GeckoBackendState;
    // Look up the affected node by (docHandle, ID) in the Rust
    // storage. Skip if it isn't in this backend's buffer. `key` is a
    // `Copy` slotmap handle, so no borrow of `state.buffer` outlives
    // this statement -- important because
    // `nvda_ia2_gecko_backend_is_root_doc_alive` below re-borrows
    // `state` internally.
    let key = match unsafe {
        (*state_ptr)
            .buffer
            .get_control_field_node_with_identifier(doc_handle, id)
    } {
        Some(k) => k,
        None => return WinEventOutcome::Continue as i32,
    };

    // If the root document accessible reports IA2_STATE_DEFUNCT,
    // the buffer is stale -- clear it. NVDA hasn't realised yet, so
    // proceeding would scribble across a different document's
    // identifier space.
    let alive = unsafe {
        nvda_ia2_gecko_backend_is_root_doc_alive(state, backend) != 0
    };
    if !alive {
        unsafe { (*state_ptr).buffer.clear() };
        return WinEventOutcome::Continue as i32;
    }

    if event_id == EVENT_OBJECT_HIDE {
        // The accessible was moved (Gecko fires hide+insert on
        // moves with a single insertion at the subtree root).
        // Force a re-render of every descendant; the parent will
        // separately fire a text-removed event so we don't need
        // to invalidate this node directly.
        if let Some(n) = unsafe { (*state_ptr).buffer.get_mut(key) } {
            if let FieldNodeKind::Control(d) = &mut n.kind {
                d.always_rerender_descendants = true;
            }
        }
        return WinEventOutcome::Continue as i32;
    }

    // Invalidate the Rust subtree, then arm the render-thread timer
    // via `requestUpdate` -- faithfully reproducing the C++
    // `invalidateSubtree` ... `requestUpdate()` tail, whose two
    // halves are split across the FFI boundary here.
    if unsafe { (*state_ptr).buffer.invalidate_subtree(key) } {
        unsafe { backend_h.request_update() };
    }
    WinEventOutcome::Continue as i32
}
