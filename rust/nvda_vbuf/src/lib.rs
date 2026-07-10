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
//!
//! # The `direct_rust_storage` feature (Phase 6d-b)
//!
//! With the feature **off** (the default) the newtypes wrap
//! `*mut c_void` and every buffer/node method forwards to the `vbuf_*`
//! externs in `c_shim.cpp` -- exactly the historical behaviour.
//!
//! With the feature **on** the newtypes wrap the Rust
//! [`storage::Buffer`] directly and the buffer/node methods call
//! `Buffer` methods -- no FFI hop. A node handle then cannot be a bare
//! pointer (Rust node identity is a 64-bit slotmap [`storage::NodeKey`]
//! and x86 pointers are 32-bit), so the node newtypes carry a
//! `(buffer, key)` back-pointer struct. [`VbufBackend`] stays routed to
//! the C-shim in both configs; its buffer-upcast / node-level methods
//! are incoherent under the feature and are therefore compiled out of
//! it (see [`VbufBackend`]). See
//! `docs/plans/2026-07-10-rust-vbuf-6db-direct-storage.md`.

use core::ffi::c_void;

#[cfg(feature = "direct_rust_storage")]
use storage::{Buffer, ControlFieldIdentifier, FieldNodeKind, NodeKey};

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

/// Phase 6d-a parallel `extern "C"` API over the Rust [`storage::Buffer`].
/// These `nvda_vbuf_*` functions coexist with the existing `vbuf_*`
/// C-shim and let callers operate on a `Box<Buffer>` without
/// crossing into C++. See module docs for conventions.
pub mod extern_api;

/// Callback invoked once with an OUT string from a `vbuf_node_get_*`
/// call. The pointer + length describe a wide-string range borrowed
/// from a `std::wstring` inside the C++ shim and is only valid for the
/// duration of the callback; copy if you need to keep it.
pub type VbufStringCallback =
    unsafe extern "C" fn(ctx: *mut c_void, ptr: *const u16, len: usize);

// ---------------------------------------------------------------------
// C-shim externs
// ---------------------------------------------------------------------

// Backend-level externs that stay routed to the C-shim in *both*
// configs (they neither construct nor consume Rust node identity).
unsafe extern "C" {
    pub fn vbuf_backend_get_root_doc_handle(backend: *mut c_void) -> i32;
    pub fn vbuf_backend_get_root_id(backend: *mut c_void) -> i32;
    pub fn vbuf_backend_clear_buffer(backend: *mut c_void);
    pub fn vbuf_backend_force_update(backend: *mut c_void);
    pub fn vbuf_backend_pending_invalid_subtrees_empty(
        backend: *mut c_void,
    ) -> i32;

    // Phase 6e (Stage A c_shim helpers). These stay routed to the
    // C-shim in *both* configs: they arm / query the render-thread
    // machinery, which is Win32-side C++ regardless of where the tree
    // is stored, and neither constructs nor consumes node identity.
    pub fn vbuf_backend_request_update(backend: *mut c_void);
    pub fn vbuf_backend_get_rust_storage_buffer(
        backend: *mut c_void,
    ) -> *mut c_void;
}

// Buffer/node externs (and the backend externs that traffic in node
// handles) are only reached when the feature is off; under the feature
// the equivalent operations run against the Rust `Buffer` instead.
#[cfg(not(feature = "direct_rust_storage"))]
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
}

// ---------------------------------------------------------------------
// Opaque-handle newtypes
// ---------------------------------------------------------------------
//
// The public method *surface* below is identical in both feature
// configs; only the wrapped handle representation and the method bodies
// differ. With the feature off the handles are raw C++ pointers; with
// it on, buffer handles are `*mut storage::Buffer` and node handles are
// a `(buffer, key)` back-pointer.

/// A `(*mut storage::Buffer, NodeKey)` back-pointer used as the node
/// handle under `direct_rust_storage`. The buffer pointer lets the
/// buffer-less node methods (`get_length`, `add_attribute`, ...) reach
/// their owning arena, matching the C++ node's implicit buffer access.
#[cfg(feature = "direct_rust_storage")]
#[derive(Clone, Copy)]
pub struct NodeRef {
    pub buffer: *mut Buffer,
    pub key: NodeKey,
}

/// A `VBufStorage_buffer_t*` (also reached as the upcast of a
/// `VBufBackend_t*` since the backend IS-A buffer).
#[cfg(not(feature = "direct_rust_storage"))]
#[derive(Clone, Copy)]
pub struct VbufBuffer(pub *mut c_void);

/// A live Rust [`storage::Buffer`]. Owned elsewhere (a `Box<Buffer>` /
/// an embedding struct); this handle only borrows it for the duration
/// of each call.
#[cfg(feature = "direct_rust_storage")]
#[derive(Clone, Copy)]
pub struct VbufBuffer(pub *mut Buffer);

/// A `VBufStorage_fieldNode_t*` (text or control field node).
#[cfg(not(feature = "direct_rust_storage"))]
#[derive(Clone, Copy)]
pub struct VbufFieldNode(pub *mut c_void);

/// A field node (text or control) identified by its buffer + slotmap
/// key.
#[cfg(feature = "direct_rust_storage")]
#[derive(Clone, Copy)]
pub struct VbufFieldNode(pub NodeRef);

/// A `VBufStorage_controlFieldNode_t*`. Subtype of `VbufFieldNode`;
/// `as_field_node()` upcasts when a field-node-only API is needed.
#[cfg(not(feature = "direct_rust_storage"))]
#[derive(Clone, Copy)]
pub struct VbufControlFieldNode(pub *mut c_void);

/// A control field node identified by its buffer + slotmap key.
/// Subtype of `VbufFieldNode`; `as_field_node()` upcasts.
#[cfg(feature = "direct_rust_storage")]
#[derive(Clone, Copy)]
pub struct VbufControlFieldNode(pub NodeRef);

/// A `VBufBackend_t*`. Stays routed to the C-shim in both feature
/// configs (Phase 6e re-homes backend orchestration onto the Rust
/// `Buffer`). Subtype of `VbufBuffer`; `as_buffer()` upcasts -- but
/// only when the feature is off, since under it a C++ backend pointer
/// is not a Rust `Buffer`.
#[derive(Clone, Copy)]
pub struct VbufBackend(pub *mut c_void);

impl VbufBuffer {
    /// Create a new control field node attached to `parent` after
    /// `previous`. Returns `None` if the underlying call failed
    /// (duplicate identifier or invalid anchor).
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            let b = unsafe { &mut *self.0 };
            let identifier = ControlFieldIdentifier { doc_handle, id };
            b.add_control_field_node(
                parent.map(|p| p.0.key),
                previous.map(|p| p.0.key),
                identifier,
                is_block,
            )
            .map(|key| {
                VbufControlFieldNode(NodeRef {
                    buffer: self.0,
                    key,
                })
            })
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            let b = unsafe { &mut *self.0 };
            b.add_text_field_node(
                parent.map(|p| p.0.key),
                previous.map(|p| p.0.key),
                text.to_vec(),
            )
            .map(|key| {
                VbufFieldNode(NodeRef {
                    buffer: self.0,
                    key,
                })
            })
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            // The referenced node's identifier and key come from its
            // own buffer; only the identifier is looked up here and the
            // key is stored verbatim for `replace_subtrees` resolution.
            let referenced = node.0;
            let identifier = unsafe { &*referenced.buffer }
                .identifier_of_control_field_node(referenced.key)?;
            let b = unsafe { &mut *self.0 };
            b.add_reference_node(
                parent.map(|p| p.0.key),
                previous.map(|p| p.0.key),
                identifier,
                referenced.key,
            )
            .map(|key| {
                VbufFieldNode(NodeRef {
                    buffer: self.0,
                    key,
                })
            })
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            let b = unsafe { &*self.0 };
            b.get_control_field_node_with_identifier(doc_handle, id).map(
                |key| {
                    VbufControlFieldNode(NodeRef {
                        buffer: self.0,
                        key,
                    })
                },
            )
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_buffer_is_descendant_node(
                    self.0,
                    parent.0,
                    descendant.0,
                ) != 0
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0 }
                .is_descendant_node(parent.0.key, descendant.0.key)
        }
    }

    /// `true` if `node` belongs to this buffer.
    ///
    /// # Safety
    ///
    /// Both handles must be live.
    pub unsafe fn is_node_in_buffer(self, node: VbufFieldNode) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_buffer_is_node_in_buffer(self.0, node.0) != 0 }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0 }.contains(node.0.key)
        }
    }
}

/// Direct-storage-only cross-buffer reuse, re-homing the
/// `VbufBackend::reuse_existing_node` path (compiled out under the
/// feature) onto the Rust `Buffer` for Phase 6e. See Decision 4 of
/// `docs/plans/2026-07-11-rust-vbuf-6e-design.md`.
#[cfg(feature = "direct_rust_storage")]
impl VbufBuffer {
    /// Look up a control field node in **this** (the live / "main")
    /// buffer that is safe to reuse while `temp` re-renders a subtree.
    /// `parent` and `previous` are nodes in `temp` (that's where the
    /// new render is going); `(doc_handle, id)` is the identifier being
    /// rendered.
    ///
    /// Delegates to [`storage::Buffer::reuse_existing_node_in_render`],
    /// whose side effect — erasing an already-invalid match from the
    /// working list so the in-flight render takes responsibility for
    /// it — mirrors the C++ `reuseExistingNodeInRender` contract.
    ///
    /// **Ownership of the returned handle:** the reused node physically
    /// lives in `self` (main), never in `temp`, so the returned
    /// [`VbufControlFieldNode`]'s `buffer` back-pointer is `self.0`.
    /// `fillVBuf` immediately feeds it to
    /// [`VbufBuffer::add_reference_node`] on `temp`, which reads the
    /// identifier from the referenced node's *own* (main) buffer and
    /// stores the main-buffer key verbatim; `replace_subtrees` later
    /// resolves that reference by moving the subtree out of main. This
    /// matches the C++ where `reuseExistingNodeInRender` returns a node
    /// belonging to `this` (the backend/main buffer) that is then
    /// handed to `temp->addReferenceNodeToBuffer`.
    ///
    /// # Safety
    ///
    /// `self` (main) and `temp` must both be live and must be **distinct**
    /// allocations (an initial render never reaches here because the
    /// render buffer *is* main). `parent` / `previous`, when `Some`,
    /// must be live nodes in `temp`.
    pub unsafe fn reuse_existing_node_in_render(
        self,
        temp: VbufBuffer,
        parent: Option<VbufControlFieldNode>,
        previous: Option<VbufFieldNode>,
        doc_handle: i32,
        id: i32,
    ) -> Option<VbufControlFieldNode> {
        debug_assert!(
            !core::ptr::eq(self.0 as *const Buffer, temp.0 as *const Buffer),
            "reuse_existing_node_in_render requires distinct main/temp \
             buffers",
        );
        let main = unsafe { &mut *self.0 };
        let temp_ref = unsafe { &*temp.0 };
        main.reuse_existing_node_in_render(
            temp_ref,
            parent.map(|p| p.0.key),
            previous.map(|p| p.0.key),
            doc_handle,
            id,
        )
        .map(|key| {
            VbufControlFieldNode(NodeRef {
                buffer: self.0,
                key,
            })
        })
    }
}

impl VbufFieldNode {
    /// Add or replace an attribute. Returns `true` on success.
    ///
    /// Under `direct_rust_storage`, a stale key returns `false` (the
    /// C++ shim would dereference a dangling pointer instead).
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn add_attribute(self, name: &[u16], value: &[u16]) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            match unsafe { &mut *self.0.buffer }.get_mut(self.0.key) {
                Some(n) => n.add_attribute(name, value),
                None => false,
            }
        }
    }

    /// Returns the attribute value as a UTF-16 `Vec`, or `None` if the
    /// attribute is absent (or, under `direct_rust_storage`, the key is
    /// stale).
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_attribute(self, name: &[u16]) -> Option<Vec<u16>> {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
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
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .get(self.0.key)
                .and_then(|n| n.get_attribute(name).map(|v| v.to_vec()))
        }
    }

    /// Returns the `name:value;...`-formatted string of every attribute
    /// (empty for a stale key under `direct_rust_storage`).
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_attributes_string(self) -> Vec<u16> {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            struct Ctx(Vec<u16>);
            unsafe extern "C" fn cb(
                ctx: *mut c_void,
                ptr: *const u16,
                len: usize,
            ) {
                let ctx = unsafe { &mut *(ctx as *mut Ctx) };
                ctx.0 =
                    unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
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
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .get(self.0.key)
                .map(|n| n.get_attributes_string())
                .unwrap_or_default()
        }
    }

    /// Rendered length of this node (`0` for a stale key under
    /// `direct_rust_storage`).
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn get_length(self) -> i32 {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_get_length(self.0) }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .get(self.0.key)
                .map(|n| n.length)
                .unwrap_or(0)
        }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn is_block(self) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_is_block(self.0) != 0 }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .get(self.0.key)
                .map(|n| n.is_block)
                .unwrap_or(false)
        }
    }

    /// Set the `isBlock` flag. No-op for a stale key under
    /// `direct_rust_storage`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn set_is_block(self, value: bool) {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_set_is_block(self.0, value as i32) }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            if let Some(n) =
                unsafe { &mut *self.0.buffer }.get_mut(self.0.key)
            {
                n.is_block = value;
            }
        }
    }

    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn is_hidden(self) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_is_hidden(self.0) != 0 }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .get(self.0.key)
                .map(|n| n.is_hidden)
                .unwrap_or(false)
        }
    }

    /// Set the `isHidden` flag. No-op for a stale key under
    /// `direct_rust_storage`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn set_is_hidden(self, value: bool) {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_set_is_hidden(self.0, value as i32) }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            if let Some(n) =
                unsafe { &mut *self.0.buffer }.get_mut(self.0.key)
            {
                n.is_hidden = value;
            }
        }
    }

    /// `true` when the node has rendered content beyond purely
    /// whitespace / private characters. See `nodeHasUsefulContent` in
    /// `nvdaHelper/vbufBase/utils.cpp`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn has_useful_content(self) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe { vbuf_node_has_useful_content(self.0) != 0 }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }.node_has_useful_content(self.0.key)
        }
    }

    /// `true` when the node's rendered text content equals `s`. See
    /// `nodeContentMatchesString` in `nvdaHelper/vbufBase/utils.cpp`.
    ///
    /// # Safety
    ///
    /// `self` must be a live field node.
    pub unsafe fn content_matches_string(self, s: &[u16]) -> bool {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_node_content_matches_string(self.0, s.as_ptr(), s.len())
                    != 0
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe { &*self.0.buffer }
                .node_content_matches_string(self.0.key, s)
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
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_node_set_always_rerender_descendants(self.0, value as i32)
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe {
                self.with_control(|d| d.always_rerender_descendants = value)
            }
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_always_rerender_children(self, value: bool) {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_node_set_always_rerender_children(self.0, value as i32)
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe {
                self.with_control(|d| d.always_rerender_children = value)
            }
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_deny_reuse_if_previous_siblings_changed(
        self,
        value: bool,
    ) {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_node_set_deny_reuse_if_previous_siblings_changed(
                    self.0,
                    value as i32,
                )
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe {
                self.with_control(|d| {
                    d.deny_reuse_if_previous_siblings_changed = value
                })
            }
        }
    }

    /// # Safety
    ///
    /// `self` must be a live control field node.
    pub unsafe fn set_requires_parent_update(self, value: bool) {
        #[cfg(not(feature = "direct_rust_storage"))]
        {
            unsafe {
                vbuf_node_set_requires_parent_update(self.0, value as i32)
            }
        }
        #[cfg(feature = "direct_rust_storage")]
        {
            unsafe {
                self.with_control(|d| d.requires_parent_update = value)
            }
        }
    }
}

/// Direct-storage-only helper: run `f` against this node's control
/// field data. No-op for a stale key or a non-control node.
#[cfg(feature = "direct_rust_storage")]
impl VbufControlFieldNode {
    #[inline]
    unsafe fn with_control<F: FnOnce(&mut storage::ControlFieldData)>(
        self,
        f: F,
    ) {
        if let Some(n) = unsafe { &mut *self.0.buffer }.get_mut(self.0.key) {
            if let FieldNodeKind::Control(d) = &mut n.kind {
                f(d);
            }
        }
    }
}

impl VbufBackend {
    /// Upcast to the buffer base type for use with `VbufBuffer` methods.
    ///
    /// Only available when `direct_rust_storage` is **off**: under the
    /// feature a `VbufBuffer` wraps a `*mut storage::Buffer`, and the
    /// C++ backend pointer is not a Rust `Buffer`, so the upcast would
    /// be a lie. Backend-as-buffer access returns in Phase 6e once
    /// backend orchestration is re-homed onto the Rust `Buffer`.
    #[cfg(not(feature = "direct_rust_storage"))]
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

    /// Ask the backend to re-render any invalid subtrees on the next
    /// render-thread tick (arms the Win32 timer via `requestUpdate`).
    /// Used by a Rust-side invalidation (Phase 6e's WinEvent dispatch)
    /// that has already invalidated the backend's Rust `storage::Buffer`
    /// directly and now needs the render thread to pick it up.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend; must be called on the render
    /// thread.
    pub unsafe fn request_update(self) {
        unsafe { vbuf_backend_request_update(self.0) }
    }

    /// The backend's embedded Rust `storage::Buffer`, or null when the
    /// backend stores its tree in the C++ `VBufStorage_buffer_t`.
    /// Returned as a raw `*mut c_void`; the caller casts to
    /// `*mut storage::Buffer`. Lets `nvda_ia2` reach the live buffer
    /// from a bare `VBufBackend_t*` where it lacks the
    /// `GeckoBackendState`.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend.
    pub unsafe fn get_rust_storage_buffer(self) -> *mut c_void {
        unsafe { vbuf_backend_get_rust_storage_buffer(self.0) }
    }

    /// Mark `node`'s subtree for re-render on the next update tick.
    ///
    /// Only available when `direct_rust_storage` is **off**: under the
    /// feature node handles are Rust keys, incompatible with the C++
    /// backend that still owns invalidation. Re-homed in Phase 6e.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend; `node` must be a live control
    /// field node owned by this backend.
    #[cfg(not(feature = "direct_rust_storage"))]
    pub unsafe fn invalidate_subtree(
        self,
        node: VbufControlFieldNode,
    ) -> bool {
        unsafe { vbuf_backend_invalidate_subtree(self.0, node.0) != 0 }
    }

    /// Look up an existing control field node on this backend that is
    /// safe to reuse during a partial re-render. Returns `None` when no
    /// matching node exists, when the backend has been told to always
    /// rerender that subtree, or when the node refused reuse.
    /// See `VBufBackend_t::reuseExistingNodeInRender` in
    /// `nvdaHelper/vbufBase/backend.cpp` for the full reuse contract.
    ///
    /// Only available when `direct_rust_storage` is **off**: under the
    /// feature node handles are Rust keys, incompatible with the C++
    /// backend. Re-homed in Phase 6e.
    ///
    /// # Safety
    ///
    /// `self` must be a live backend; `parent` and `previous`, when
    /// `Some`, must be live nodes belonging to a buffer in mid-render.
    #[cfg(not(feature = "direct_rust_storage"))]
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

// =====================================================================
// Direct-storage wrapper tests
// =====================================================================
//
// These exercise the newtype surface end-to-end against the *real*
// Rust `Buffer` (only reachable when the feature is on; otherwise the
// methods would need the C++ shim, which a stand-alone test binary
// can't link).

#[cfg(all(test, feature = "direct_rust_storage"))]
mod direct_tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Owns a heap `Buffer` behind a raw pointer, re-`Box`ing it on
    /// drop so the wrapper tests free without leaking.
    struct OwnedBuffer(*mut Buffer);
    impl OwnedBuffer {
        fn new() -> Self {
            Self(Box::into_raw(Box::new(Buffer::new())))
        }
        fn handle(&self) -> VbufBuffer {
            VbufBuffer(self.0)
        }
    }
    impl Drop for OwnedBuffer {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.0) });
        }
    }

    #[test]
    fn builds_tree_adds_nodes_and_grows_length() {
        let ob = OwnedBuffer::new();
        let buf = ob.handle();
        unsafe {
            let root = buf
                .add_control_field_node(None, None, 1, 1, true)
                .expect("root");
            let text = buf
                .add_text_field_node(
                    Some(root),
                    None,
                    &w("hello"),
                )
                .expect("text");
            // Length bubbles up to the ancestor control node.
            assert_eq!(text.get_length(), 5);
            assert_eq!(root.as_field_node().get_length(), 5);
            // Duplicate identifier is rejected.
            assert!(
                buf.add_control_field_node(
                    Some(root),
                    Some(text),
                    1,
                    1,
                    false,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn attributes_round_trip() {
        let ob = OwnedBuffer::new();
        let buf = ob.handle();
        unsafe {
            let root = buf
                .add_control_field_node(None, None, 1, 1, false)
                .expect("root");
            let node = root.as_field_node();
            assert!(node.add_attribute(&w("role"), &w("button")));
            assert_eq!(node.get_attribute(&w("role")), Some(w("button")));
            assert_eq!(node.get_attribute(&w("absent")), None);
            assert_eq!(node.get_attributes_string(), w("role:button;"));
            // Overwrite.
            assert!(node.add_attribute(&w("role"), &w("link")));
            assert_eq!(node.get_attribute(&w("role")), Some(w("link")));
        }
    }

    #[test]
    fn block_and_hidden_flags_round_trip() {
        let ob = OwnedBuffer::new();
        let buf = ob.handle();
        unsafe {
            let root = buf
                .add_control_field_node(None, None, 1, 1, true)
                .expect("root");
            let node = root.as_field_node();
            assert!(node.is_block());
            node.set_is_block(false);
            assert!(!node.is_block());
            assert!(!node.is_hidden());
            node.set_is_hidden(true);
            assert!(node.is_hidden());
            // Control-field rerender flags don't panic.
            root.set_always_rerender_descendants(true);
            root.set_always_rerender_children(true);
            root.set_deny_reuse_if_previous_siblings_changed(true);
            root.set_requires_parent_update(true);
        }
    }

    #[test]
    fn lookup_descendant_and_membership() {
        let ob = OwnedBuffer::new();
        let buf = ob.handle();
        unsafe {
            let root = buf
                .add_control_field_node(None, None, 1, 1, true)
                .expect("root");
            let inner = buf
                .add_control_field_node(Some(root), None, 1, 2, false)
                .expect("inner");
            // Identifier lookup hit / miss.
            let found = buf
                .get_control_field_node_with_identifier(1, 2)
                .expect("found");
            assert_eq!(found.0.key, inner.0.key);
            assert!(
                buf.get_control_field_node_with_identifier(9, 9).is_none()
            );
            // Descendant relation is directional.
            assert!(buf.is_descendant_node(
                root.as_field_node(),
                inner.as_field_node(),
            ));
            assert!(!buf.is_descendant_node(
                inner.as_field_node(),
                root.as_field_node(),
            ));
            // Membership.
            assert!(buf.is_node_in_buffer(inner.as_field_node()));
        }
    }

    #[test]
    fn content_queries_match_text() {
        let ob = OwnedBuffer::new();
        let buf = ob.handle();
        unsafe {
            let root = buf
                .add_control_field_node(None, None, 1, 1, true)
                .expect("root");
            let text = buf
                .add_text_field_node(Some(root), None, &w("hello"))
                .expect("text");
            assert!(text.content_matches_string(&w("hello")));
            assert!(!text.content_matches_string(&w("world")));
            assert!(text.has_useful_content());
        }
    }

    #[test]
    fn reference_node_aliases_control_identifier() {
        // A temp buffer can reference a control node living in a
        // separate (main) buffer by its identifier.
        let main = OwnedBuffer::new();
        let temp = OwnedBuffer::new();
        unsafe {
            let target = main
                .handle()
                .add_control_field_node(None, None, 1, 7, false)
                .expect("target");
            let temp_root = temp
                .handle()
                .add_control_field_node(None, None, 2, 1, true)
                .expect("temp root");
            let reference = temp
                .handle()
                .add_reference_node(Some(temp_root), None, target)
                .expect("reference");
            // The reference is a live node in the temp buffer.
            assert!(temp
                .handle()
                .is_node_in_buffer(reference));
        }
    }

    #[test]
    fn reuse_existing_node_returns_main_node() {
        // Main holds a rendered tree: root (1,1) -> child (1,2).
        // Temp is mid-render with a fresh root (1,1). Reusing (1,2)
        // returns a handle pointing INTO main at the existing child.
        let main = OwnedBuffer::new();
        let temp = OwnedBuffer::new();
        unsafe {
            let root = main
                .handle()
                .add_control_field_node(None, None, 1, 1, true)
                .expect("main root");
            let child = main
                .handle()
                .add_control_field_node(Some(root), None, 1, 2, false)
                .expect("main child");
            main.handle()
                .add_text_field_node(Some(child), None, &w("hi"))
                .expect("child text");

            let temp_root = temp
                .handle()
                .add_control_field_node(None, None, 1, 1, true)
                .expect("temp root");

            let reused = main
                .handle()
                .reuse_existing_node_in_render(
                    temp.handle(),
                    Some(temp_root),
                    None,
                    1,
                    2,
                )
                .expect("reuse");
            // The returned handle owns into MAIN, not temp, and
            // identifies the existing child.
            assert_eq!(reused.0.buffer, main.0);
            assert_eq!(reused.0.key, child.0.key);
            // Feeding it to add_reference_node on temp builds a live
            // reference node there (the fillVBuf block1 sequence).
            let reference = temp
                .handle()
                .add_reference_node(Some(temp_root), None, reused)
                .expect("reference");
            assert!(temp.handle().is_node_in_buffer(reference));
        }
    }

    #[test]
    fn reuse_existing_node_erases_from_working_and_refuses() {
        // When the candidate is already in main's working list (it was
        // invalidated this tick), reuse refuses (returns None) AND
        // removes it from working so the in-flight render owns it.
        let main = OwnedBuffer::new();
        let temp = OwnedBuffer::new();
        unsafe {
            let root = main
                .handle()
                .add_control_field_node(None, None, 1, 1, true)
                .expect("main root");
            let child = main
                .handle()
                .add_control_field_node(Some(root), None, 1, 2, false)
                .expect("main child");
            // Invalidate the child and promote pending -> working.
            // Each statement takes a fresh, short-lived borrow so no
            // `&mut` outlives the reuse call below.
            (*main.0).invalidate_subtree(child.0.key);
            (*main.0).take_pending_into_working();
            assert!(!(*main.0).working_invalid_empty());

            let temp_root = temp
                .handle()
                .add_control_field_node(None, None, 1, 1, true)
                .expect("temp root");

            let reused = main.handle().reuse_existing_node_in_render(
                temp.handle(),
                Some(temp_root),
                None,
                1,
                2,
            );
            assert!(reused.is_none());
            // The candidate was erased from the working list.
            assert!((*main.0).working_invalid_empty());
        }
    }
}
