//! Per-instance Rust state and C-ABI entry points for
//! `AdobeAcrobatVBufBackend_t`, mirroring
//! `nvda_ia2::gecko_backend_state`.
//!
//! The C++ backend holds a `void* rustState` (allocated in its
//! constructor, freed in its destructor) and routes its storage-bearing
//! operations here: `update()` drives the shared
//! `nvda_vbuf::backend::run_raw_update` over the embedded `Buffer` using
//! the Acrobat [`fill_vbuf`] renderer; `getRustStorageBuffer()` exposes
//! the `Buffer` for the vbufRemote read RPCs; and the render-thread
//! win-event hook routes its node lookup + invalidation here. The
//! render-thread machinery itself (timer, hook registration) stays C++.

use core::ffi::c_void;

use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IServiceProvider;
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromEvent, IAccessible,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

use nvda_vbuf::backend::run_raw_update;
use nvda_vbuf::storage::Buffer;
use nvda_vbuf::{VbufBackend, VbufBuffer};

use crate::fill_vbuf::{
    fill_vbuf, get_pddom_node, service_provider, u, FillVBufCtx,
};
use crate::interfaces::IPDDomDocPagination;

/// `STATE_SYSTEM_READONLY` (oleacc.h).
const STATE_SYSTEM_READONLY: i32 = 0x40;

/// Per-instance state owned by Rust, exposed to the C++ class as `void*`.
pub struct AcrobatBackendState {
    /// The live Acrobat storage tree; rendered into and read out of.
    pub buffer: Buffer,
    /// Whether the document is an XFA form. Computed once on the initial
    /// render (mirrors the C++ `!oldNode` branch of `render`) and reused.
    pub is_xfa: bool,
    /// The document's pagination interface (for page labels). Computed on
    /// the initial render and reused across re-renders.
    pub doc_pagination: Option<IPDDomDocPagination>,
}

impl AcrobatBackendState {
    fn new() -> Self {
        Self {
            buffer: Buffer::new(),
            // C++ initialises isXFA = true.
            is_xfa: true,
            doc_pagination: None,
        }
    }
}

/// Resolve a `(docHandle, id)` pair to an MSAA `IAccessible` via
/// `AccessibleObjectFromEvent`. Port of the C++
/// `IAccessibleFromIdentifier`.
///
/// # Safety
///
/// The COM apartment must be initialised; `doc_handle` is reinterpreted
/// as an `HWND`.
unsafe fn from_identifier(doc_handle: i32, id: i32) -> Option<IAccessible> {
    let hwnd = HWND(doc_handle as isize as *mut c_void);
    let mut pacc: Option<IAccessible> = None;
    let mut varchild = VARIANT::default();
    if unsafe {
        AccessibleObjectFromEvent(
            hwnd,
            OBJID_CLIENT.0 as u32,
            id as u32,
            &mut pacc,
            &mut varchild,
        )
    }
    .is_err()
    {
        return None;
    }
    // varchild drops here (VariantClear via Drop), matching the C++
    // VariantClear.
    pacc
}

/// Port of C++ `checkIsXFA`: a document is *not* XFA if its root
/// accessible is read-only.
unsafe fn check_is_xfa(pacc: &IAccessible, varchild: &VARIANT) -> bool {
    let states = match unsafe { pacc.get_accState(varchild) } {
        Ok(v) => {
            let raw = v.as_raw();
            let vt = unsafe { raw.Anonymous.Anonymous.vt };
            // VT_I4 = 3.
            if vt == 3 {
                unsafe { raw.Anonymous.Anonymous.Anonymous.lVal }
            } else {
                0
            }
        }
        Err(_) => return false,
    };
    (states & STATE_SYSTEM_READONLY) == 0
}

/// Port of C++ `getDocPagination`: `IAccessible` -> `IServiceProvider`
/// -> `IPDDomNode` -> `IPDDomDocPagination`.
unsafe fn get_doc_pagination(
    pacc: &IAccessible,
    varchild: &VARIANT,
) -> Option<IPDDomDocPagination> {
    let servprov: IServiceProvider = service_provider(pacc)?;
    let dom_node = get_pddom_node(varchild, &servprov)?;
    dom_node.cast().ok()
}

/// Language inherited by a re-rendered subtree's root: the
/// `acrobat::language` attribute stored on the old node's parent in
/// `main` (empty if none). Mirrors the C++ `oldNode->getParent()->language`
/// inheritance.
unsafe fn inherited_lang_for_old_node(
    main: *mut Buffer,
    old_node: nvda_vbuf::storage::NodeKey,
) -> Vec<u16> {
    let node = unsafe { VbufBuffer(main).control_field_node(old_node) };
    match unsafe { node.parent() } {
        Some(parent) => unsafe {
            parent
                .as_field_node()
                .get_attribute(&u("acrobat::language"))
        }
        .unwrap_or_default(),
        None => Vec::new(),
    }
}

// --- C-ABI entry points ---------------------------------------------------

/// Allocate an [`AcrobatBackendState`] and return a raw `void*`. Pair
/// with [`acrobat_backend_destroy`].
#[no_mangle]
pub extern "C" fn acrobat_backend_create() -> *mut c_void {
    Box::into_raw(Box::new(AcrobatBackendState::new())) as *mut c_void
}

/// Free an [`AcrobatBackendState`]. Accepts `NULL` as a no-op.
///
/// # Safety
///
/// `state` must be `NULL` or a pointer from [`acrobat_backend_create`]
/// not yet destroyed.
#[no_mangle]
pub unsafe extern "C" fn acrobat_backend_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(state as *mut AcrobatBackendState) });
}

/// Address of this backend's embedded `Buffer`, for
/// `AdobeAcrobatVBufBackend_t::getRustStorageBuffer`.
///
/// # Safety
///
/// `state` must be a valid `AcrobatBackendState*`; the returned pointer
/// is valid while the state lives and must be dereferenced only under the
/// backend lock.
#[no_mangle]
pub unsafe extern "C" fn acrobat_backend_get_buffer(
    state: *mut c_void,
) -> *mut Buffer {
    if state.is_null() {
        return core::ptr::null_mut();
    }
    let state = unsafe { &mut *(state as *mut AcrobatBackendState) };
    &mut state.buffer as *mut Buffer
}

/// Empty this backend's `Buffer` (render-thread terminate / new document).
///
/// # Safety
///
/// `state` must be a valid `AcrobatBackendState*`.
#[no_mangle]
pub unsafe extern "C" fn acrobat_backend_clear_buffer(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut AcrobatBackendState) };
    state.buffer.clear();
}

/// Drain/render/merge orchestration over the embedded `Buffer`. Backs
/// `AdobeAcrobatVBufBackend_t::update()`. Returns `true` when the caller
/// should fire `vbufChangeNotify` (re-render branch), `false` on the
/// initial render.
///
/// # Safety
///
/// * `state` must be a valid `AcrobatBackendState*`.
/// * `backend` must be a valid `VBufBackend_t*`.
/// * Must run on the render thread with the backend lock held.
#[no_mangle]
pub unsafe extern "C" fn acrobat_backend_update(
    state: *mut c_void,
    backend: *mut c_void,
) -> bool {
    if state.is_null() || backend.is_null() {
        return false;
    }
    let state = unsafe { &mut *(state as *mut AcrobatBackendState) };
    let backend_h = VbufBackend(backend);
    let root_doc_handle = unsafe { backend_h.root_doc_handle() };
    let root_id = unsafe { backend_h.root_id() };

    // Initial render (mirrors C++ `render`'s `!oldNode` branch): compute
    // isXFA + docPagination from the root accessible before rendering.
    // Must happen while we still hold `&mut state`, before `main_ptr`.
    if !state.buffer.has_content() {
        if let Some(pacc) = unsafe { from_identifier(root_doc_handle, root_id) }
        {
            let varchild = VARIANT::from(0i32);
            state.is_xfa = unsafe { check_is_xfa(&pacc, &varchild) };
            state.doc_pagination =
                unsafe { get_doc_pagination(&pacc, &varchild) };
        }
    }
    let is_xfa = state.is_xfa;
    let doc_pagination = state.doc_pagination.clone();

    // From here `state.buffer` is reached only through `main_ptr`.
    let main_ptr: *mut Buffer = &mut state.buffer as *mut Buffer;
    unsafe {
        run_raw_update(
            main_ptr,
            root_doc_handle,
            root_id,
            |target, main, doc_handle, id, old_node| {
                let pacc = match from_identifier(doc_handle, id) {
                    Some(p) => p,
                    None => return false,
                };
                // Seed the re-rendered subtree's root language from the
                // old node's parent (empty on the initial render).
                let inherited_lang = match old_node {
                    Some(k) => inherited_lang_for_old_node(main, k),
                    None => Vec::new(),
                };
                let ctx = FillVBufCtx {
                    doc_handle,
                    is_xfa,
                    doc_pagination: doc_pagination.clone(),
                };
                fill_vbuf(
                    &pacc,
                    VbufBuffer(target),
                    None,
                    None,
                    &inherited_lang,
                    None,
                    &ctx,
                );
                true
            },
        )
    }
}

/// Render-thread win-event tail: look up `(docHandle, id)` in this
/// backend's `Buffer` and, if present, invalidate its subtree and arm the
/// update timer. Port of the storage tail of the C++
/// `renderThread_winEventProcHook` (`getControlFieldNodeWithIdentifier`
/// + `invalidateSubtree`), routed to the Rust storage.
///
/// # Safety
///
/// `state` must be a valid `AcrobatBackendState*`; `backend` a valid
/// `VBufBackend_t*`.
#[no_mangle]
pub unsafe extern "C" fn acrobat_backend_invalidate_node(
    state: *mut c_void,
    backend: *mut c_void,
    doc_handle: i32,
    id: i32,
) {
    if state.is_null() || backend.is_null() {
        return;
    }
    let state = unsafe { &mut *(state as *mut AcrobatBackendState) };
    let backend_h = VbufBackend(backend);
    let key = match state
        .buffer
        .get_control_field_node_with_identifier(doc_handle, id)
    {
        Some(k) => k,
        None => return,
    };
    if state.buffer.invalidate_subtree(key) {
        unsafe { backend_h.request_update() };
    }
}
