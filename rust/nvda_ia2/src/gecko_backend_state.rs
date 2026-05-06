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
use nvda_vbuf::{VbufBackend, VbufBuffer};

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
}

impl GeckoBackendState {
    pub fn new() -> Self {
        Self {
            toolkit_name: Vec::new(),
            root_doc_acc: None,
        }
    }

    /// `true` when the cached toolkit name is `"Chrome"`. Used by
    /// `fillVBuf` for the IA2 `relationTargetsOfType` workaround.
    pub fn is_chrome(&self) -> bool {
        self.toolkit_name == "Chrome".encode_utf16().collect::<Vec<u16>>()
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
    state.toolkit_name = get_toolkit_name_native(acc);
}

/// C-callable replacement for `GeckoVBufBackend_t::render`. Resolves
/// the `(doc_handle, id)` pair to an `IAccessible2`, runs
/// `versionSpecificInit` when this is the root call (`is_root_call`),
/// and then invokes `fill_vbuf` to populate the buffer.
///
/// `is_root_call` is `true` when the C++ side passes a NULL `oldNode`
/// (i.e. this is the top-level render rather than a partial update).
///
/// The acquired `IAccessible2` is released when the function returns,
/// matching the C++ original's `pacc->Release()` cleanup.
///
/// # Safety
///
/// * `state` must be a valid `GeckoBackendState*`.
/// * `backend` must be a valid `VBufBackend_t*`.
/// * `buffer` must be a valid `VBufStorage_buffer_t*`.
/// * Caller must hold the render-thread invariants vbufBase requires.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_gecko_backend_render(
    state: *mut c_void,
    backend: *mut c_void,
    buffer: *mut c_void,
    doc_handle: i32,
    id: i32,
    is_root_call: bool,
    root_id: i32,
) {
    if state.is_null() || backend.is_null() || buffer.is_null() {
        return;
    }
    let acc = match unsafe { from_identifier(doc_handle, id) } {
        Some(a) => a,
        None => return,
    };
    if is_root_call {
        let state_mut = unsafe { &mut *(state as *mut GeckoBackendState) };
        state_mut.toolkit_name = get_toolkit_name_native(&acc);
    }
    let is_chrome = unsafe { &*(state as *const GeckoBackendState) }.is_chrome();

    let ctx = FillVBufCtx {
        backend: VbufBackend(backend),
        root_id,
        is_chrome,
    };
    // Top-level call: no parent / previous / table state inherited.
    let _ = unsafe {
        fill_vbuf(
            &acc,
            VbufBuffer(buffer),
            None,
            None,
            None,
            0,
            None,
            false,
            &ctx,
        )
    };
    // `acc` drops here -- IAccessible2's Drop runs Release, balancing
    // the AddRef from from_identifier.
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
    // Pending update -> short-circuit alive.
    let backend_h = VbufBackend(backend);
    if !unsafe { backend_h.pending_invalid_subtrees_empty() } {
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
