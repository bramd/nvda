//! Phase 6d-a: parallel `extern "C"` API over the Rust `Buffer`
//! storage in [`crate::storage`].
//!
//! These `nvda_vbuf_*` functions coexist with the existing `vbuf_*`
//! C-shim (which still wraps the live C++ `VBufStorage_buffer_t`).
//! The `nvda_vbuf_*` surface operates on `Box<Buffer>` -- pure Rust,
//! no C++ involvement -- and is intended for callers that want the
//! Rust storage path. Phase 6d-b will route the existing `nvda_vbuf`
//! crate's newtype wrappers to these functions behind a feature flag;
//! Phase 6e will switch gecko_ia2 onto the Rust storage live.
//!
//! ## Conventions
//!
//! * **Buffer pointer:** `*mut Buffer` returned by
//!   [`nvda_vbuf_buffer_create`], destroyed by
//!   [`nvda_vbuf_buffer_destroy`]. Never call any other function with
//!   a buffer that has been destroyed.
//! * **Node identity across the FFI:** a `u64` produced by
//!   [`slotmap::KeyData::as_ffi`]. `0` is the null sentinel; real
//!   keys never encode to `0` because slotmap versions are
//!   `NonZeroU32`, so any valid key has its high 32 bits non-zero.
//! * **Booleans:** `i32` (`0` / non-zero), matching the existing
//!   C-shim's `vbuf_*` signatures.
//! * **Strings:** `(ptr, len)` pairs of UTF-16; OUT strings come
//!   back through [`NvdaVbufStringCallback`] so the caller controls
//!   allocation. Empty strings are invoked with `len == 0`.
//! * **Threading:** all operations are synchronous on the caller's
//!   thread. The C-shim's same thread-affinity contract applies.
//!
//! ## What's NOT yet exposed
//!
//! * `force_update` and the full backend update orchestration. Those
//!   require a polymorphic [`crate::backend::Renderer`] which is
//!   awkward to thread through the FFI; gecko_ia2's migration in
//!   Phase 6e will pick the right shape.
//! * `replace_subtrees`. Same reason -- it consumes owned temp
//!   `Buffer`s and is exercised through `update`.

use core::ffi::c_void;

use slotmap::{Key, KeyData};

use crate::storage::{
    Buffer, ControlFieldIdentifier, FieldNodeKind, FindDirection, NodeKey,
};

// ---------------------------------------------------------------------
// String OUT callback (matches the existing `vbuf_string_callback`)
// ---------------------------------------------------------------------

/// Invoked once per OUT-string-bearing call when a value is present.
/// `ptr` + `len` describe a UTF-16 range owned by the Rust buffer
/// and only valid for the duration of the callback; copy if you need
/// to keep it.
pub type NvdaVbufStringCallback =
    unsafe extern "C" fn(ctx: *mut c_void, ptr: *const u16, len: usize);

// ---------------------------------------------------------------------
// Key encoding
// ---------------------------------------------------------------------

/// `0` represents a missing / failed `NodeKey` over the FFI.
pub const NVDA_VBUF_NODE_NONE: u64 = 0;

#[inline]
fn key_to_ffi(key: NodeKey) -> u64 {
    key.data().as_ffi()
}

/// Decode an FFI key into a `NodeKey`. Returns `None` for the null
/// sentinel; otherwise the caller is still responsible for handling
/// stale-key cases via `Buffer::contains` (slotmap will return `None`
/// on lookup of a stale key).
#[inline]
fn ffi_to_key(value: u64) -> Option<NodeKey> {
    if value == NVDA_VBUF_NODE_NONE {
        None
    } else {
        Some(KeyData::from_ffi(value).into())
    }
}

// ---------------------------------------------------------------------
// Buffer-as-pointer helpers
// ---------------------------------------------------------------------

#[inline]
unsafe fn buf_ref<'a>(p: *const Buffer) -> &'a Buffer {
    debug_assert!(!p.is_null());
    unsafe { &*p }
}

#[inline]
unsafe fn buf_mut<'a>(p: *mut Buffer) -> &'a mut Buffer {
    debug_assert!(!p.is_null());
    unsafe { &mut *p }
}

// =====================================================================
// Buffer lifecycle
// =====================================================================

/// Create a fresh empty buffer. Caller owns the returned pointer and
/// must release it with [`nvda_vbuf_buffer_destroy`].
#[unsafe(no_mangle)]
pub extern "C" fn nvda_vbuf_buffer_create() -> *mut Buffer {
    Box::into_raw(Box::new(Buffer::new()))
}

/// Drop a buffer previously returned by [`nvda_vbuf_buffer_create`].
/// Passing a null pointer is a no-op.
///
/// # Safety
///
/// `buffer` must either be null or a pointer previously returned by
/// [`nvda_vbuf_buffer_create`] that has not yet been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_destroy(buffer: *mut Buffer) {
    if buffer.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(buffer) });
}

/// Empty `buffer` (drop every node, reset selection, drop pending /
/// working invalid lists). Mirrors the C++ `clearBuffer`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_clear(buffer: *mut Buffer) {
    unsafe { buf_mut(buffer) }.clear();
}

/// Total rendered length of `buffer` (zero when empty).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_text_length(
    buffer: *const Buffer,
) -> i32 {
    unsafe { buf_ref(buffer) }.text_length()
}

/// `1` when `buffer` has a root, `0` otherwise.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_has_content(
    buffer: *const Buffer,
) -> i32 {
    unsafe { buf_ref(buffer) }.has_content() as i32
}

// =====================================================================
// Node insertion
// =====================================================================

/// Add a control field node. `parent` / `previous` are FFI keys (or
/// `0` for "none"). Returns the new node's FFI key, or `0` on
/// failure (duplicate identifier, invalid anchor).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `parent` / `previous`,
/// when non-zero, must be keys belonging to this buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_add_control_field_node(
    buffer: *mut Buffer,
    parent: u64,
    previous: u64,
    doc_handle: i32,
    id: i32,
    is_block: i32,
) -> u64 {
    let b = unsafe { buf_mut(buffer) };
    let identifier = ControlFieldIdentifier { doc_handle, id };
    match b.add_control_field_node(
        ffi_to_key(parent),
        ffi_to_key(previous),
        identifier,
        is_block != 0,
    ) {
        Some(k) => key_to_ffi(k),
        None => NVDA_VBUF_NODE_NONE,
    }
}

/// Add a text field node. `text_ptr` + `text_len` is a UTF-16 range.
/// Returns the new node's FFI key, or `0` on failure.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `parent` / `previous`,
/// when non-zero, must belong to this buffer; `text_ptr` must point
/// to at least `text_len` `u16` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_add_text_field_node(
    buffer: *mut Buffer,
    parent: u64,
    previous: u64,
    text_ptr: *const u16,
    text_len: usize,
) -> u64 {
    let b = unsafe { buf_mut(buffer) };
    let text =
        unsafe { core::slice::from_raw_parts(text_ptr, text_len) }.to_vec();
    match b.add_text_field_node(
        ffi_to_key(parent),
        ffi_to_key(previous),
        text,
    ) {
        Some(k) => key_to_ffi(k),
        None => NVDA_VBUF_NODE_NONE,
    }
}

/// Add a reference node aliasing the control field identified by
/// `(referenced_doc_handle, referenced_id)`. The reference's own
/// identifier matches the referenced node. `referenced_key` is the
/// FFI key of the target in (typically) the backend's main buffer
/// and is stored verbatim for resolution at `replace_subtrees` time;
/// it must not be `0`.
///
/// Returns the new node's FFI key, or `0` on failure.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_add_reference_node(
    buffer: *mut Buffer,
    parent: u64,
    previous: u64,
    referenced_doc_handle: i32,
    referenced_id: i32,
    referenced_key: u64,
) -> u64 {
    let b = unsafe { buf_mut(buffer) };
    let target = match ffi_to_key(referenced_key) {
        Some(k) => k,
        None => return NVDA_VBUF_NODE_NONE,
    };
    let identifier = ControlFieldIdentifier {
        doc_handle: referenced_doc_handle,
        id: referenced_id,
    };
    match b.add_reference_node(
        ffi_to_key(parent),
        ffi_to_key(previous),
        identifier,
        target,
    ) {
        Some(k) => key_to_ffi(k),
        None => NVDA_VBUF_NODE_NONE,
    }
}

// =====================================================================
// Tree queries
// =====================================================================

/// Look up a control field by `(docHandle, id)`. Returns `0` when
/// not found.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_get_control_field_node_with_identifier(
    buffer: *const Buffer,
    doc_handle: i32,
    id: i32,
) -> u64 {
    match unsafe { buf_ref(buffer) }
        .get_control_field_node_with_identifier(doc_handle, id)
    {
        Some(k) => key_to_ffi(k),
        None => NVDA_VBUF_NODE_NONE,
    }
}

/// `1` if `descendant` is reachable from `parent` via the parent
/// chain (strictly below; `parent == descendant` is `0`).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; both keys, when non-zero,
/// should belong to this buffer (otherwise the call returns `0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_is_descendant_node(
    buffer: *const Buffer,
    parent: u64,
    descendant: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let (Some(p), Some(d)) = (ffi_to_key(parent), ffi_to_key(descendant))
    else {
        return 0;
    };
    b.is_descendant_node(p, d) as i32
}

/// `1` if `key` is in `buffer`'s arena (i.e. not stale).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_is_node_in_buffer(
    buffer: *const Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    match ffi_to_key(key) {
        Some(k) => b.contains(k) as i32,
        None => 0,
    }
}

/// Find the deepest control (or reference) field containing `offset`.
/// On success, returns the parent's FFI key and writes the parent's
/// `(start, end)` offsets and `(docHandle, id)` to the OUT params if
/// non-null. Returns `0` on failure.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; OUT params, when non-null,
/// must point to writable `i32` storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_locate_control_field_node_at_offset(
    buffer: *const Buffer,
    offset: i32,
    out_start: *mut i32,
    out_end: *mut i32,
    out_doc_handle: *mut i32,
    out_id: *mut i32,
) -> u64 {
    let b = unsafe { buf_ref(buffer) };
    let r = match b.locate_control_field_node_at_offset(offset) {
        Some(r) => r,
        None => return NVDA_VBUF_NODE_NONE,
    };
    unsafe {
        if !out_start.is_null() {
            *out_start = r.start;
        }
        if !out_end.is_null() {
            *out_end = r.end;
        }
        if !out_doc_handle.is_null() {
            *out_doc_handle = r.doc_handle;
        }
        if !out_id.is_null() {
            *out_id = r.id;
        }
    }
    key_to_ffi(r.node)
}

/// Find a field node whose attributes match `regexp`, searching from
/// `offset` in `direction` (`0` = forward, `1` = back, `2` = up,
/// matching `VBufStorage_findDirection_t`). `offset` of `-1` searches
/// from the root of the buffer.
///
/// `attribs_ptr` + `attribs_len` is the whitespace-separated
/// attribute-name list (UTF-16); `regexp_ptr` + `regexp_len` is the
/// match pattern (UTF-16). On a hit, returns the node's FFI key and
/// writes its `(start, end)` offsets to the OUT params (when
/// non-null). Returns `0` (and leaves the OUT params untouched) on no
/// match, an unknown `direction`, an invalid `offset`, or a regex
/// that fails to compile.
///
/// Mirrors `VBufRemote_findNodeByAttributes` /
/// `VBufStorage_buffer_t::findNodeByAttributes`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `attribs_ptr` /
/// `regexp_ptr` must each point to at least their declared lengths of
/// `u16`; OUT params, when non-null, must point to writable `i32`
/// storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_find_node_by_attributes(
    buffer: *const Buffer,
    offset: i32,
    direction: i32,
    attribs_ptr: *const u16,
    attribs_len: usize,
    regexp_ptr: *const u16,
    regexp_len: usize,
    out_start: *mut i32,
    out_end: *mut i32,
) -> u64 {
    let b = unsafe { buf_ref(buffer) };
    let dir = match direction {
        0 => FindDirection::Forward,
        1 => FindDirection::Back,
        2 => FindDirection::Up,
        _ => return NVDA_VBUF_NODE_NONE,
    };
    let attribs =
        unsafe { core::slice::from_raw_parts(attribs_ptr, attribs_len) };
    let regexp =
        unsafe { core::slice::from_raw_parts(regexp_ptr, regexp_len) };
    let r = match b.find_node_by_attributes(offset, dir, attribs, regexp) {
        Some(r) => r,
        None => return NVDA_VBUF_NODE_NONE,
    };
    unsafe {
        if !out_start.is_null() {
            *out_start = r.start;
        }
        if !out_end.is_null() {
            *out_end = r.end;
        }
    }
    key_to_ffi(r.node)
}

/// Read the `(start, end)` offsets of `key` in the buffer's text.
/// Returns `1` on success (and writes the OUT params), `0` on stale
/// key.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_field_node_offsets(
    buffer: *const Buffer,
    key: u64,
    out_start: *mut i32,
    out_end: *mut i32,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let (start, end) = match b.field_node_offsets(key) {
        Some(p) => p,
        None => return 0,
    };
    unsafe {
        if !out_start.is_null() {
            *out_start = start;
        }
        if !out_end.is_null() {
            *out_end = end;
        }
    }
    1
}

/// `1` when `offset` falls within `key`'s rendered range.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_is_field_node_at_offset(
    buffer: *const Buffer,
    key: u64,
    offset: i32,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    match ffi_to_key(key) {
        Some(k) => b.is_field_node_at_offset(k, offset) as i32,
        None => 0,
    }
}

/// Read the `(docHandle, id)` of a control or reference node.
/// Returns `1` on success, `0` for stale or text-kind keys.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_identifier(
    buffer: *const Buffer,
    key: u64,
    out_doc_handle: *mut i32,
    out_id: *mut i32,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let identifier = match b.identifier_of_control_field_node(key) {
        Some(i) => i,
        None => return 0,
    };
    unsafe {
        if !out_doc_handle.is_null() {
            *out_doc_handle = identifier.doc_handle;
        }
        if !out_id.is_null() {
            *out_id = identifier.id;
        }
    }
    1
}

// =====================================================================
// Text retrieval
// =====================================================================

/// Pull the text in `[start_offset, end_offset)`. When `use_markup`
/// is non-zero the text is wrapped in vbuf XML tags (matching
/// `getTextInRange` with `useMarkup=true`).
///
/// On success the callback is invoked once with the text; returns
/// `1`. When the buffer is empty or the range is invalid, the
/// callback is not invoked and the return is `0`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `cb` must be safe to call
/// with the given `ctx`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_get_text_in_range(
    buffer: *const Buffer,
    start_offset: i32,
    end_offset: i32,
    use_markup: i32,
    ctx: *mut c_void,
    cb: NvdaVbufStringCallback,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let mut out: Vec<u16> = Vec::new();
    if !b.buffer_get_text_in_range(
        start_offset,
        end_offset,
        &mut out,
        use_markup != 0,
    ) {
        return 0;
    }
    unsafe { cb(ctx, out.as_ptr(), out.len()) };
    1
}

// =====================================================================
// Selection / line offsets
// =====================================================================

/// Read the current `(start, end)` selection range. Always `1`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_get_selection_offsets(
    buffer: *const Buffer,
    out_start: *mut i32,
    out_end: *mut i32,
) -> i32 {
    let (s, e) = unsafe { buf_ref(buffer) }.selection_offsets();
    unsafe {
        if !out_start.is_null() {
            *out_start = s;
        }
        if !out_end.is_null() {
            *out_end = e;
        }
    }
    1
}

/// Set the selection range. Returns `1` on success, `0` for an
/// invalid (negative or inverted) range.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_set_selection_offsets(
    buffer: *mut Buffer,
    start_offset: i32,
    end_offset: i32,
) -> i32 {
    unsafe { buf_mut(buffer) }
        .set_selection_offsets(start_offset, end_offset) as i32
}

/// Compute the start/end of the line containing `offset`. Returns
/// `1` on success, `0` for an empty buffer or out-of-range offset.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_line_offsets(
    buffer: *const Buffer,
    offset: i32,
    max_line_length: i32,
    use_screen_layout: i32,
    out_start: *mut i32,
    out_end: *mut i32,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let (s, e) = match b.line_offsets(
        offset,
        max_line_length,
        use_screen_layout != 0,
    ) {
        Some(p) => p,
        None => return 0,
    };
    unsafe {
        if !out_start.is_null() {
            *out_start = s;
        }
        if !out_end.is_null() {
            *out_end = e;
        }
    }
    1
}

// =====================================================================
// Node attributes
// =====================================================================

/// Add or overwrite an attribute on `key`. Returns `1` on success
/// (or replacement), `0` for a stale key.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `name_ptr` / `value_ptr`
/// must point to at least their declared lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_add_attribute(
    buffer: *mut Buffer,
    key: u64,
    name_ptr: *const u16,
    name_len: usize,
    value_ptr: *const u16,
    value_len: usize,
) -> i32 {
    let b = unsafe { buf_mut(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let n = match b.get_mut(key) {
        Some(n) => n,
        None => return 0,
    };
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let value =
        unsafe { core::slice::from_raw_parts(value_ptr, value_len) };
    n.add_attribute(name, value) as i32
}

/// Look up an attribute by name. When present, invokes `cb` once
/// with the value and returns `1`; when absent, leaves `cb`
/// uncalled and returns `0`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `name_ptr` must point to
/// at least `name_len` `u16` elements; `cb` must be safe to call
/// with the given `ctx`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_get_attribute(
    buffer: *const Buffer,
    key: u64,
    name_ptr: *const u16,
    name_len: usize,
    ctx: *mut c_void,
    cb: NvdaVbufStringCallback,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let n = match b.get(key) {
        Some(n) => n,
        None => return 0,
    };
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    match n.get_attribute(name) {
        Some(value) => {
            unsafe { cb(ctx, value.as_ptr(), value.len()) };
            1
        }
        None => 0,
    }
}

/// Invoke `cb` once with the `name:value;...` serialization of every
/// attribute. Returns `1` on success, `0` for a stale key (in which
/// case `cb` is not invoked).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `cb` must be safe to call
/// with the given `ctx`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_get_attributes_string(
    buffer: *const Buffer,
    key: u64,
    ctx: *mut c_void,
    cb: NvdaVbufStringCallback,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let n = match b.get(key) {
        Some(n) => n,
        None => return 0,
    };
    let s = n.get_attributes_string();
    unsafe { cb(ctx, s.as_ptr(), s.len()) };
    1
}

/// Length of `key`'s rendered text. Returns `0` for a stale key
/// (matching the C++ behaviour where the call would have already
/// dereferenced a dangling pointer).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_get_length(
    buffer: *const Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    b.get(key).map(|n| n.length).unwrap_or(0)
}

/// `1` if `key`'s `is_block` flag is set, `0` otherwise (or for
/// stale keys).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_is_block(
    buffer: *const Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    b.get(key).map(|n| n.is_block as i32).unwrap_or(0)
}

/// Set `key`'s `is_block` flag. No-op for stale keys.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_is_block(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    let b = unsafe { buf_mut(buffer) };
    if let Some(k) = ffi_to_key(key) {
        if let Some(n) = b.get_mut(k) {
            n.is_block = value != 0;
        }
    }
}

/// `1` if `key`'s `is_hidden` flag is set, `0` otherwise (or stale).
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_is_hidden(
    buffer: *const Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    b.get(key).map(|n| n.is_hidden as i32).unwrap_or(0)
}

/// Set `key`'s `is_hidden` flag. No-op for stale keys.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_is_hidden(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    let b = unsafe { buf_mut(buffer) };
    if let Some(k) = ffi_to_key(key) {
        if let Some(n) = b.get_mut(k) {
            n.is_hidden = value != 0;
        }
    }
}

/// `1` when `key` has rendered content beyond purely whitespace /
/// private characters. See `Buffer::node_has_useful_content`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_has_useful_content(
    buffer: *const Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    match ffi_to_key(key) {
        Some(k) => b.node_has_useful_content(k) as i32,
        None => 0,
    }
}

/// `1` when `key`'s rendered text equals the given UTF-16 string.
/// See `Buffer::node_content_matches_string`.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer; `str_ptr` must point to
/// at least `str_len` `u16` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_content_matches_string(
    buffer: *const Buffer,
    key: u64,
    str_ptr: *const u16,
    str_len: usize,
) -> i32 {
    let b = unsafe { buf_ref(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    let s = unsafe { core::slice::from_raw_parts(str_ptr, str_len) };
    b.node_content_matches_string(key, s) as i32
}

// =====================================================================
// Control-field rerender flags
// =====================================================================

fn with_control_field<F: FnOnce(&mut crate::storage::ControlFieldData)>(
    buffer: *mut Buffer,
    key: u64,
    f: F,
) {
    let b = unsafe { buf_mut(buffer) };
    let Some(k) = ffi_to_key(key) else {
        return;
    };
    if let Some(n) = b.get_mut(k) {
        if let FieldNodeKind::Control(d) = &mut n.kind {
            f(d);
        }
    }
}

/// Set `alwaysRerenderDescendants` on a control field. No-op for
/// stale keys or non-control nodes.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_always_rerender_descendants(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    with_control_field(buffer, key, |d| {
        d.always_rerender_descendants = value != 0;
    });
}

/// Set `alwaysRerenderChildren` on a control field. No-op for stale
/// keys or non-control nodes.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_always_rerender_children(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    with_control_field(buffer, key, |d| {
        d.always_rerender_children = value != 0;
    });
}

/// Set `denyReuseIfPreviousSiblingsChanged` on a control field.
/// No-op for stale keys or non-control nodes.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_deny_reuse_if_previous_siblings_changed(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    with_control_field(buffer, key, |d| {
        d.deny_reuse_if_previous_siblings_changed = value != 0;
    });
}

/// Set `requiresParentUpdate` on a control field. No-op for stale
/// keys or non-control nodes.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_node_set_requires_parent_update(
    buffer: *mut Buffer,
    key: u64,
    value: i32,
) {
    with_control_field(buffer, key, |d| {
        d.requires_parent_update = value != 0;
    });
}

// =====================================================================
// Backend-side helpers (storage's own pieces)
// =====================================================================

/// Mark a control field for re-render on the next update tick.
/// Returns `1` on success, `0` for a stale key.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_invalidate_subtree(
    buffer: *mut Buffer,
    key: u64,
) -> i32 {
    let b = unsafe { buf_mut(buffer) };
    let key = match ffi_to_key(key) {
        Some(k) => k,
        None => return 0,
    };
    b.invalidate_subtree(key) as i32
}

/// `1` when no pending invalid subtrees are queued.
///
/// # Safety
///
/// `buffer` must be a live buffer pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_pending_invalid_subtrees_empty(
    buffer: *const Buffer,
) -> i32 {
    unsafe { buf_ref(buffer) }.pending_invalid_subtrees_empty() as i32
}

/// Try to reuse an existing main-buffer node during a temp-buffer
/// render. `main` is the backend's main buffer (the one being
/// updated); `temp` is the in-flight render's temp buffer that is
/// supplying `parent` and `previous`. Returns the existing node's
/// FFI key (in `main`) on success, `0` otherwise.
///
/// # Safety
///
/// `main` and `temp` must both be live buffer pointers; they may
/// point at the same buffer (the `&mut self` / `&Buffer` borrow
/// rule means we forbid that case here -- caller must pass distinct
/// pointers).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_vbuf_buffer_reuse_existing_node(
    main: *mut Buffer,
    temp: *const Buffer,
    parent: u64,
    previous: u64,
    doc_handle: i32,
    id: i32,
) -> u64 {
    debug_assert!(
        !core::ptr::eq(main as *const Buffer, temp),
        "reuse_existing_node requires distinct main/temp buffers",
    );
    let main_b = unsafe { buf_mut(main) };
    let temp_b = unsafe { buf_ref(temp) };
    match main_b.reuse_existing_node_in_render(
        temp_b,
        ffi_to_key(parent),
        ffi_to_key(previous),
        doc_handle,
        id,
    ) {
        Some(k) => key_to_ffi(k),
        None => NVDA_VBUF_NODE_NONE,
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A scoped `Box<Buffer>` raw pointer that is freed on drop, so
    /// tests don't need explicit destroy calls.
    struct OwnedBuffer(*mut Buffer);
    impl OwnedBuffer {
        fn new() -> Self {
            Self(nvda_vbuf_buffer_create())
        }
        fn ptr(&self) -> *mut Buffer {
            self.0
        }
    }
    impl Drop for OwnedBuffer {
        fn drop(&mut self) {
            unsafe { nvda_vbuf_buffer_destroy(self.0) };
        }
    }

    /// `NvdaVbufStringCallback` that copies the value into a `Vec<u16>`
    /// captured by the context.
    unsafe extern "C" fn collect_cb(
        ctx: *mut c_void,
        ptr: *const u16,
        len: usize,
    ) {
        let out = unsafe { &mut *(ctx as *mut Vec<u16>) };
        *out = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
    }

    #[test]
    fn create_destroy_is_safe() {
        let b = nvda_vbuf_buffer_create();
        assert!(!b.is_null());
        unsafe {
            assert_eq!(nvda_vbuf_buffer_text_length(b), 0);
            assert_eq!(nvda_vbuf_buffer_has_content(b), 0);
            nvda_vbuf_buffer_destroy(b);
            // null destroy is fine
            nvda_vbuf_buffer_destroy(core::ptr::null_mut());
        }
    }

    #[test]
    fn add_control_then_text_returns_keys_and_grows_buffer() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            assert_ne!(root, NVDA_VBUF_NODE_NONE);
            let text = w("hello");
            let t = nvda_vbuf_buffer_add_text_field_node(
                b,
                root,
                0,
                text.as_ptr(),
                text.len(),
            );
            assert_ne!(t, NVDA_VBUF_NODE_NONE);
            assert_eq!(nvda_vbuf_buffer_text_length(b), 5);
            assert_eq!(nvda_vbuf_buffer_has_content(b), 1);
            // Duplicate identifier returns 0.
            let dup = nvda_vbuf_buffer_add_control_field_node(
                b, root, t, 1, 1, 0,
            );
            assert_eq!(dup, NVDA_VBUF_NODE_NONE);
        }
    }

    #[test]
    fn lookup_by_identifier_and_offsets() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let inner = nvda_vbuf_buffer_add_control_field_node(
                b, root, 0, 1, 2, 0,
            );
            let text = w("abc");
            let _t = nvda_vbuf_buffer_add_text_field_node(
                b,
                inner,
                0,
                text.as_ptr(),
                text.len(),
            );
            // Lookup hit and miss.
            assert_eq!(
                nvda_vbuf_buffer_get_control_field_node_with_identifier(
                    b, 1, 2,
                ),
                inner
            );
            assert_eq!(
                nvda_vbuf_buffer_get_control_field_node_with_identifier(
                    b, 9, 9,
                ),
                NVDA_VBUF_NODE_NONE
            );
            // is_descendant and is_node_in_buffer.
            assert_eq!(nvda_vbuf_buffer_is_descendant_node(b, root, inner), 1);
            assert_eq!(nvda_vbuf_buffer_is_descendant_node(b, inner, root), 0);
            assert_eq!(nvda_vbuf_buffer_is_node_in_buffer(b, inner), 1);
            assert_eq!(nvda_vbuf_buffer_is_node_in_buffer(b, 0), 0);
            // field_node_offsets.
            let mut s = -1i32;
            let mut e = -1i32;
            assert_eq!(
                nvda_vbuf_buffer_field_node_offsets(b, inner, &mut s, &mut e),
                1
            );
            assert_eq!((s, e), (0, 3));
            // identifier_of.
            let mut dh = 0i32;
            let mut id = 0i32;
            assert_eq!(nvda_vbuf_node_identifier(b, inner, &mut dh, &mut id), 1);
            assert_eq!((dh, id), (1, 2));
        }
    }

    #[test]
    fn locate_control_field_at_offset_writes_out_params() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let inner = nvda_vbuf_buffer_add_control_field_node(
                b, root, 0, 1, 2, 0,
            );
            let text = w("xy");
            let _ = nvda_vbuf_buffer_add_text_field_node(
                b,
                inner,
                0,
                text.as_ptr(),
                text.len(),
            );
            let (mut s, mut e, mut dh, mut id) = (-1, -1, -1, -1);
            let key = nvda_vbuf_buffer_locate_control_field_node_at_offset(
                b, 0, &mut s, &mut e, &mut dh, &mut id,
            );
            assert_eq!(key, inner);
            assert_eq!((s, e, dh, id), (0, 2, 1, 2));
            // Out of range -> 0.
            assert_eq!(
                nvda_vbuf_buffer_locate_control_field_node_at_offset(
                    b,
                    99,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ),
                NVDA_VBUF_NODE_NONE
            );
        }
    }

    #[test]
    fn get_text_in_range_invokes_callback() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let txt = w("hello");
            let _ = nvda_vbuf_buffer_add_text_field_node(
                b,
                root,
                0,
                txt.as_ptr(),
                txt.len(),
            );
            let mut out: Vec<u16> = Vec::new();
            let r = nvda_vbuf_buffer_get_text_in_range(
                b,
                0,
                5,
                0,
                &mut out as *mut _ as *mut c_void,
                collect_cb,
            );
            assert_eq!(r, 1);
            assert_eq!(out, w("hello"));
        }
    }

    #[test]
    fn selection_and_line_offsets_round_trip() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let txt = w("hello\nworld");
            let _ = nvda_vbuf_buffer_add_text_field_node(
                b,
                root,
                0,
                txt.as_ptr(),
                txt.len(),
            );
            // Selection.
            assert_eq!(
                nvda_vbuf_buffer_set_selection_offsets(b, 2, 5),
                1
            );
            let (mut s, mut e) = (-1, -1);
            assert_eq!(
                nvda_vbuf_buffer_get_selection_offsets(b, &mut s, &mut e),
                1
            );
            assert_eq!((s, e), (2, 5));
            assert_eq!(
                nvda_vbuf_buffer_set_selection_offsets(b, 5, 2),
                0
            );
            // Line offsets: offset 0 -> first line "hello\n" ends
            // at 6 (line break inclusive).
            let (mut ls, mut le) = (-1, -1);
            assert_eq!(
                nvda_vbuf_buffer_line_offsets(
                    b, 0, 0, 1, &mut ls, &mut le,
                ),
                1
            );
            assert_eq!((ls, le), (0, 6));
        }
    }

    #[test]
    fn attribute_round_trip_via_callback() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 0,
            );
            let name = w("role");
            let value = w("button");
            assert_eq!(
                nvda_vbuf_node_add_attribute(
                    b,
                    root,
                    name.as_ptr(),
                    name.len(),
                    value.as_ptr(),
                    value.len(),
                ),
                1
            );
            let mut out: Vec<u16> = Vec::new();
            assert_eq!(
                nvda_vbuf_node_get_attribute(
                    b,
                    root,
                    name.as_ptr(),
                    name.len(),
                    &mut out as *mut _ as *mut c_void,
                    collect_cb,
                ),
                1
            );
            assert_eq!(out, w("button"));
            // Missing attribute -> 0, no callback.
            let absent = w("nothere");
            let mut out2: Vec<u16> = Vec::new();
            assert_eq!(
                nvda_vbuf_node_get_attribute(
                    b,
                    root,
                    absent.as_ptr(),
                    absent.len(),
                    &mut out2 as *mut _ as *mut c_void,
                    collect_cb,
                ),
                0
            );
            assert!(out2.is_empty());
            // Attributes string includes the role we added.
            let mut all: Vec<u16> = Vec::new();
            assert_eq!(
                nvda_vbuf_node_get_attributes_string(
                    b,
                    root,
                    &mut all as *mut _ as *mut c_void,
                    collect_cb,
                ),
                1
            );
            assert_eq!(all, w("role:button;"));
        }
    }

    #[test]
    fn block_and_hidden_setters() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 0,
            );
            assert_eq!(nvda_vbuf_node_is_block(b, root), 0);
            nvda_vbuf_node_set_is_block(b, root, 1);
            assert_eq!(nvda_vbuf_node_is_block(b, root), 1);
            assert_eq!(nvda_vbuf_node_is_hidden(b, root), 0);
            nvda_vbuf_node_set_is_hidden(b, root, 1);
            assert_eq!(nvda_vbuf_node_is_hidden(b, root), 1);
        }
    }

    #[test]
    fn invalidate_and_pending_subtree_query() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let inner = nvda_vbuf_buffer_add_control_field_node(
                b, root, 0, 1, 2, 0,
            );
            assert_eq!(
                nvda_vbuf_buffer_pending_invalid_subtrees_empty(b),
                1
            );
            assert_eq!(nvda_vbuf_buffer_invalidate_subtree(b, inner), 1);
            assert_eq!(
                nvda_vbuf_buffer_pending_invalid_subtrees_empty(b),
                0
            );
            // Stale key -> 0.
            assert_eq!(nvda_vbuf_buffer_invalidate_subtree(b, 0), 0);
        }
    }

    #[test]
    fn stale_key_after_clear_returns_falsy() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            nvda_vbuf_buffer_clear(b);
            assert_eq!(nvda_vbuf_buffer_is_node_in_buffer(b, root), 0);
            assert_eq!(nvda_vbuf_node_get_length(b, root), 0);
            assert_eq!(nvda_vbuf_node_is_block(b, root), 0);
            // No-op setters on stale keys are harmless (don't panic).
            nvda_vbuf_node_set_is_block(b, root, 1);
            nvda_vbuf_node_set_always_rerender_descendants(b, root, 1);
        }
    }

    #[test]
    fn find_node_by_attributes_round_trip() {
        let ob = OwnedBuffer::new();
        let b = ob.ptr();
        unsafe {
            // root(block) > c1(role=heading)>"Title" , c2(role=link)>"link"
            let root = nvda_vbuf_buffer_add_control_field_node(
                b, 0, 0, 1, 1, 1,
            );
            let c1 = nvda_vbuf_buffer_add_control_field_node(
                b, root, 0, 1, 2, 0,
            );
            let role = w("role");
            let heading = w("heading");
            nvda_vbuf_node_add_attribute(
                b,
                c1,
                role.as_ptr(),
                role.len(),
                heading.as_ptr(),
                heading.len(),
            );
            let title = w("Title");
            let _ = nvda_vbuf_buffer_add_text_field_node(
                b,
                c1,
                0,
                title.as_ptr(),
                title.len(),
            );
            let c2 = nvda_vbuf_buffer_add_control_field_node(
                b, root, c1, 1, 3, 0,
            );
            let link = w("link");
            nvda_vbuf_node_add_attribute(
                b,
                c2,
                role.as_ptr(),
                role.len(),
                link.as_ptr(),
                link.len(),
            );
            let txt = w("link");
            let _ = nvda_vbuf_buffer_add_text_field_node(
                b,
                c2,
                0,
                txt.as_ptr(),
                txt.len(),
            );

            let attribs = w("role");
            let regexp = w("role:(?:heading;)");
            let (mut start, mut end) = (-1i32, -1i32);
            // Forward from root (offset -1) finds c1 (the heading).
            let found = nvda_vbuf_buffer_find_node_by_attributes(
                b,
                -1,
                0, // forward
                attribs.as_ptr(),
                attribs.len(),
                regexp.as_ptr(),
                regexp.len(),
                &mut start,
                &mut end,
            );
            assert_eq!(found, c1);
            assert_eq!((start, end), (0, 5));

            // Unknown direction -> NONE, OUT params untouched.
            let (mut s2, mut e2) = (-7i32, -7i32);
            let none = nvda_vbuf_buffer_find_node_by_attributes(
                b,
                -1,
                9, // invalid direction
                attribs.as_ptr(),
                attribs.len(),
                regexp.as_ptr(),
                regexp.len(),
                &mut s2,
                &mut e2,
            );
            assert_eq!(none, NVDA_VBUF_NODE_NONE);
            assert_eq!((s2, e2), (-7, -7));

            // No-match pattern -> NONE.
            let no_regexp = w("role:(?:banner;)");
            let miss = nvda_vbuf_buffer_find_node_by_attributes(
                b,
                -1,
                0,
                attribs.as_ptr(),
                attribs.len(),
                no_regexp.as_ptr(),
                no_regexp.len(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            assert_eq!(miss, NVDA_VBUF_NODE_NONE);
        }
    }

    #[test]
    fn key_round_trip_is_stable() {
        // Sanity check: encoding then decoding a real key gives the
        // same `NodeKey`.
        let mut buf = Buffer::new();
        let k = buf
            .add_control_field_node(
                None,
                None,
                ControlFieldIdentifier {
                    doc_handle: 1,
                    id: 1,
                },
                false,
            )
            .unwrap();
        let raw = key_to_ffi(k);
        assert_ne!(raw, NVDA_VBUF_NODE_NONE);
        assert_eq!(ffi_to_key(raw), Some(k));
        assert_eq!(ffi_to_key(NVDA_VBUF_NODE_NONE), None);
    }
}

