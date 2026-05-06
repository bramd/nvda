//! Rust bindings over `nvdaHelper/vbufBase/c_shim.cpp`.
//!
//! Opaque-handle newtypes around the C++ `VBufStorage_*` and
//! `VBufBackend_t` classes; method wrappers are `unsafe` because the
//! underlying pointer's lifetime and thread-affinity are the caller's
//! responsibility -- they match the C++ contract that vbuf operations
//! happen on the render thread holding a valid backend pointer.
//!
//! This crate is consumed by `nvda_ia2` (and future Rust vbuf
//! backends). It produces only an `rlib`; the underlying `extern "C"`
//! symbols are defined in `c_shim.cpp` and reach `nvdaHelperRemote.dll`
//! through whichever staticlib crate ultimately links into the DLL.

use core::ffi::c_void;

#[cfg(feature = "test_stubs")]
mod test_stubs;

/// Phase 6 in-progress port of `nvdaHelper/vbufBase/storage.cpp`
/// to Rust. Not yet wired into the C-shim; the existing C++ storage
/// is still the live implementation reached through `c_shim.cpp`.
pub mod storage;

/// Phase 6c in-progress port of `nvdaHelper/vbufBase/backend.cpp`'s
/// orchestration logic. Currently exposes the polymorphic
/// [`backend::Renderer`] trait and the [`backend::update`]
/// orchestration function. Win32 render-thread machinery (timer,
/// hooks, `execInThread`) is still TODO and stays C++-side for now.
pub mod backend;

/// Callback invoked once with an OUT string from a `vbuf_node_get_*`
/// call. The pointer + length describe a wide-string range borrowed
/// from a `std::wstring` inside the C++ shim and is only valid for the
/// duration of the callback; copy if you need to keep it.
pub type VbufStringCallback =
    unsafe extern "C" fn(ctx: *mut c_void, ptr: *const u16, len: usize);

unsafe extern "C" {
    pub fn vbuf_buffer_add_control_field_node(
        buffer: *mut c_void,
        parent: *mut c_void,
        previous: *mut c_void,
        doc_handle: i32,
        id: i32,
        is_block: i32,
    ) -> *mut c_void;

    pub fn vbuf_buffer_add_text_field_node(
        buffer: *mut c_void,
        parent: *mut c_void,
        previous: *mut c_void,
        text_ptr: *const u16,
        text_len: usize,
    ) -> *mut c_void;

    pub fn vbuf_buffer_add_reference_node(
        buffer: *mut c_void,
        parent: *mut c_void,
        previous: *mut c_void,
        node: *mut c_void,
    ) -> *mut c_void;

    pub fn vbuf_buffer_get_control_field_node_with_identifier(
        buffer: *mut c_void,
        doc_handle: i32,
        id: i32,
    ) -> *mut c_void;

    pub fn vbuf_buffer_is_descendant_node(
        buffer: *mut c_void,
        parent: *mut c_void,
        descendant: *mut c_void,
    ) -> i32;

    pub fn vbuf_buffer_is_node_in_buffer(
        buffer: *mut c_void,
        node: *mut c_void,
    ) -> i32;

    pub fn vbuf_node_add_attribute(
        node: *mut c_void,
        name_ptr: *const u16,
        name_len: usize,
        value_ptr: *const u16,
        value_len: usize,
    ) -> i32;

    pub fn vbuf_node_get_attribute(
        node: *mut c_void,
        name_ptr: *const u16,
        name_len: usize,
        ctx: *mut c_void,
        cb: VbufStringCallback,
    ) -> i32;

    pub fn vbuf_node_get_attributes_string(
        node: *mut c_void,
        ctx: *mut c_void,
        cb: VbufStringCallback,
    );

    pub fn vbuf_node_get_length(node: *mut c_void) -> i32;
    pub fn vbuf_node_is_block(node: *mut c_void) -> i32;
    pub fn vbuf_node_set_is_block(node: *mut c_void, value: i32);
    pub fn vbuf_node_is_hidden(node: *mut c_void) -> i32;
    pub fn vbuf_node_set_is_hidden(node: *mut c_void, value: i32);
    pub fn vbuf_node_has_useful_content(node: *mut c_void) -> i32;
    pub fn vbuf_node_content_matches_string(
        node: *mut c_void,
        str_ptr: *const u16,
        str_len: usize,
    ) -> i32;

    pub fn vbuf_node_set_always_rerender_descendants(
        node: *mut c_void,
        value: i32,
    );
    pub fn vbuf_node_set_always_rerender_children(
        node: *mut c_void,
        value: i32,
    );
    pub fn vbuf_node_set_deny_reuse_if_previous_siblings_changed(
        node: *mut c_void,
        value: i32,
    );
    pub fn vbuf_node_set_requires_parent_update(
        node: *mut c_void,
        value: i32,
    );

    pub fn vbuf_backend_get_root_doc_handle(backend: *mut c_void) -> i32;
    pub fn vbuf_backend_get_root_id(backend: *mut c_void) -> i32;
    pub fn vbuf_backend_clear_buffer(backend: *mut c_void);
    pub fn vbuf_backend_force_update(backend: *mut c_void);
    pub fn vbuf_backend_invalidate_subtree(
        backend: *mut c_void,
        node: *mut c_void,
    ) -> i32;

    pub fn vbuf_backend_reuse_existing_node(
        backend: *mut c_void,
        parent: *mut c_void,
        previous: *mut c_void,
        doc_handle: i32,
        id: i32,
    ) -> *mut c_void;

    pub fn vbuf_backend_pending_invalid_subtrees_empty(
        backend: *mut c_void,
    ) -> i32;
}

// ---------------------------------------------------------------------
// Opaque-handle newtypes
// ---------------------------------------------------------------------

/// A `VBufStorage_buffer_t*` (also reached as the upcast of a
/// `VBufBackend_t*` since the backend IS-A buffer).
#[derive(Clone, Copy)]
pub struct VbufBuffer(pub *mut c_void);

/// A `VBufStorage_fieldNode_t*` (text or control field node).
#[derive(Clone, Copy)]
pub struct VbufFieldNode(pub *mut c_void);

/// A `VBufStorage_controlFieldNode_t*`. Subtype of `VbufFieldNode`;
/// `as_field_node()` upcasts when a field-node-only API is needed.
#[derive(Clone, Copy)]
pub struct VbufControlFieldNode(pub *mut c_void);

/// A `VBufBackend_t*`. Subtype of `VbufBuffer`; `as_buffer()` upcasts
/// when a buffer-level API is needed.
#[derive(Clone, Copy)]
pub struct VbufBackend(pub *mut c_void);

impl VbufBuffer {
    /// Create a new control field node attached to `parent` after
    /// `previous`. Returns `None` if the underlying call failed.
    ///
    /// # Safety
    ///
    /// All non-null handles must point to live nodes in this buffer.
    pub unsafe fn add_control_field_node(
        self,
        parent: Option<VbufControlFieldNode>,
        previous: Option<VbufFieldNode>,
        doc_handle: i32,
        id: i32,
        is_block: bool,
    ) -> Option<VbufControlFieldNode> {
        let raw = unsafe {
            vbuf_buffer_add_control_field_node(
                self.0,
                parent.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                previous.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                doc_handle,
                id,
                is_block as i32,
            )
        };
        if raw.is_null() {
            None
        } else {
            Some(VbufControlFieldNode(raw))
        }
    }

    /// Create a new text field node containing `text` (UTF-16) attached
    /// to `parent` after `previous`. Returns `None` on failure.
    ///
    /// # Safety
    ///
    /// All non-null handles must point to live nodes in this buffer.
    pub unsafe fn add_text_field_node(
        self,
        parent: Option<VbufControlFieldNode>,
        previous: Option<VbufFieldNode>,
        text: &[u16],
    ) -> Option<VbufFieldNode> {
        let raw = unsafe {
            vbuf_buffer_add_text_field_node(
                self.0,
                parent.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                previous.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                text.as_ptr(),
                text.len(),
            )
        };
        if raw.is_null() {
            None
        } else {
            Some(VbufFieldNode(raw))
        }
    }

    /// Add a reference node copying from an existing control field node.
    ///
    /// # Safety
    ///
    /// All non-null handles must point to live nodes; `node` may be
    /// from a different buffer (that's the use case).
    pub unsafe fn add_reference_node(
        self,
        parent: Option<VbufControlFieldNode>,
        previous: Option<VbufFieldNode>,
        node: VbufControlFieldNode,
    ) -> Option<VbufFieldNode> {
        let raw = unsafe {
            vbuf_buffer_add_reference_node(
                self.0,
                parent.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                previous.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                node.0,
            )
        };
        if raw.is_null() {
            None
        } else {
            Some(VbufFieldNode(raw))
        }
    }

    /// Look up a control field node by `(docHandle, ID)`.
    ///
    /// # Safety
    ///
    /// Buffer must be live.
    pub unsafe fn get_control_field_node_with_identifier(
        self,
        doc_handle: i32,
        id: i32,
    ) -> Option<VbufControlFieldNode> {
        let raw = unsafe {
            vbuf_buffer_get_control_field_node_with_identifier(
                self.0,
                doc_handle,
                id,
            )
        };
        if raw.is_null() {
            None
        } else {
            Some(VbufControlFieldNode(raw))
        }
    }

    /// `true` if `descendant` is reachable through `parent`'s subtree.
    ///
    /// # Safety
    ///
    /// All handles must be live and belong to this buffer.
    pub unsafe fn is_descendant_node(
        self,
        parent: VbufFieldNode,
        descendant: VbufFieldNode,
    ) -> bool {
        unsafe {
            vbuf_buffer_is_descendant_node(self.0, parent.0, descendant.0) != 0
        }
    }

    /// `true` if `node` belongs to this buffer.
    ///
    /// # Safety
    ///
    /// Both handles must be live.
    pub unsafe fn is_node_in_buffer(self, node: VbufFieldNode) -> bool {
        unsafe { vbuf_buffer_is_node_in_buffer(self.0, node.0) != 0 }
    }
}

impl VbufFieldNode {
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn add_attribute(self, name: &[u16], value: &[u16]) -> bool {
        unsafe {
            vbuf_node_add_attribute(
                self.0,
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
            ) != 0
        }
    }

    /// Returns the attribute value as a UTF-16 `Vec`, or `None` if the
    /// attribute is absent.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_attribute(self, name: &[u16]) -> Option<Vec<u16>> {
        struct Ctx(Option<Vec<u16>>);
        unsafe extern "C" fn cb(
            ctx: *mut c_void,
            ptr: *const u16,
            len: usize,
        ) {
            let ctx = unsafe { &mut *(ctx as *mut Ctx) };
            ctx.0 = Some(
                unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec(),
            );
        }
        let mut ctx = Ctx(None);
        let found = unsafe {
            vbuf_node_get_attribute(
                self.0,
                name.as_ptr(),
                name.len(),
                &mut ctx as *mut _ as *mut c_void,
                cb,
            )
        };
        if found != 0 {
            ctx.0
        } else {
            None
        }
    }

    /// Returns the `name:value;...`-formatted string of every attribute.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_attributes_string(self) -> Vec<u16> {
        struct Ctx(Vec<u16>);
        unsafe extern "C" fn cb(
            ctx: *mut c_void,
            ptr: *const u16,
            len: usize,
        ) {
            let ctx = unsafe { &mut *(ctx as *mut Ctx) };
            ctx.0 = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
        }
        let mut ctx = Ctx(Vec::new());
        unsafe {
            vbuf_node_get_attributes_string(
                self.0,
                &mut ctx as *mut _ as *mut c_void,
                cb,
            );
        }
        ctx.0
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_length(self) -> i32 {
        unsafe { vbuf_node_get_length(self.0) }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn is_block(self) -> bool {
        unsafe { vbuf_node_is_block(self.0) != 0 }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn set_is_block(self, value: bool) {
        unsafe { vbuf_node_set_is_block(self.0, value as i32) }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn is_hidden(self) -> bool {
        unsafe { vbuf_node_is_hidden(self.0) != 0 }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn set_is_hidden(self, value: bool) {
        unsafe { vbuf_node_set_is_hidden(self.0, value as i32) }
    }

    /// `true` when the node has rendered content beyond purely
    /// whitespace / private characters. See `nodeHasUsefulContent` in
    /// `nvdaHelper/vbufBase/utils.cpp`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn has_useful_content(self) -> bool {
        unsafe { vbuf_node_has_useful_content(self.0) != 0 }
    }

    /// `true` when the node's rendered text content equals `s`. See
    /// `nodeContentMatchesString` in `nvdaHelper/vbufBase/utils.cpp`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn content_matches_string(self, s: &[u16]) -> bool {
        unsafe {
            vbuf_node_content_matches_string(self.0, s.as_ptr(), s.len()) != 0
        }
    }
}

impl VbufControlFieldNode {
    /// Upcast to the field-node base type for use with `VbufFieldNode`
    /// methods.
    pub fn as_field_node(self) -> VbufFieldNode {
        VbufFieldNode(self.0)
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_always_rerender_descendants(self, value: bool) {
        unsafe {
            vbuf_node_set_always_rerender_descendants(self.0, value as i32)
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_always_rerender_children(self, value: bool) {
        unsafe {
            vbuf_node_set_always_rerender_children(self.0, value as i32)
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_deny_reuse_if_previous_siblings_changed(
        self,
        value: bool,
    ) {
        unsafe {
            vbuf_node_set_deny_reuse_if_previous_siblings_changed(
                self.0,
                value as i32,
            )
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_requires_parent_update(self, value: bool) {
        unsafe {
            vbuf_node_set_requires_parent_update(self.0, value as i32)
        }
    }
}

impl VbufBackend {
    /// Upcast to the buffer base type for use with `VbufBuffer` methods.
    pub fn as_buffer(self) -> VbufBuffer {
        VbufBuffer(self.0)
    }

    /// # Safety
    ///
    /// `self` must be a live backend.
    pub unsafe fn root_doc_handle(self) -> i32 {
        unsafe { vbuf_backend_get_root_doc_handle(self.0) }
    }

    /// # Safety
    ///
    /// `self` must be a live backend.
    pub unsafe fn root_id(self) -> i32 {
        unsafe { vbuf_backend_get_root_id(self.0) }
    }

    /// # Safety
    ///
    /// `self` must be a live backend; must be called on the render
    /// thread.
    pub unsafe fn clear_buffer(self) {
        unsafe { vbuf_backend_clear_buffer(self.0) }
    }

    /// # Safety
    ///
    /// `self` must be a live backend; must be called on the render
    /// thread.
    pub unsafe fn force_update(self) {
        unsafe { vbuf_backend_force_update(self.0) }
    }

    /// # Safety
    ///
    /// `self` must be a live backend; `node` must be a live control
    /// field node owned by this backend.
    pub unsafe fn invalidate_subtree(
        self,
        node: VbufControlFieldNode,
    ) -> bool {
        unsafe { vbuf_backend_invalidate_subtree(self.0, node.0) != 0 }
    }

    /// `true` when the backend has no pending invalid subtrees waiting
    /// to be re-rendered. Used to short-circuit `isRootDocAlive`'s COM
    /// check when an update is already pending.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend.
    pub unsafe fn pending_invalid_subtrees_empty(self) -> bool {
        unsafe { vbuf_backend_pending_invalid_subtrees_empty(self.0) != 0 }
    }

    /// Look up an existing control field node on this backend that is
    /// safe to reuse during a partial re-render. Returns `None` when no
    /// matching node exists, when the backend has been told to always
    /// rerender that subtree, or when the node refused reuse.
    /// See `VBufBackend_t::reuseExistingNodeInRender` in
    /// `nvdaHelper/vbufBase/backend.cpp` for the full reuse contract.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend; `parent` and `previous`, when
    /// `Some`, must be live nodes belonging to a buffer in mid-render.
    pub unsafe fn reuse_existing_node(
        self,
        parent: Option<VbufControlFieldNode>,
        previous: Option<VbufFieldNode>,
        doc_handle: i32,
        id: i32,
    ) -> Option<VbufControlFieldNode> {
        let raw = unsafe {
            vbuf_backend_reuse_existing_node(
                self.0,
                parent.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                previous.map(|p| p.0).unwrap_or(core::ptr::null_mut()),
                doc_handle,
                id,
            )
        };
        if raw.is_null() {
            None
        } else {
            Some(VbufControlFieldNode(raw))
        }
    }
}
