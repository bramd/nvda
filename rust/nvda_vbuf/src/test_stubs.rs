//! Stub definitions of every `vbuf_*` extern "C" symbol declared in
//! `lib.rs`. Compiled only when the `test_stubs` feature is active —
//! see `Cargo.toml` for the rationale.
//!
//! Each stub panics so that any test that *actually* exercises a vbuf
//! call fails loudly. Tests in downstream crates that don't touch vbuf
//! at all just need these to satisfy the linker.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::VbufStringCallback;

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_add_control_field_node(
    _buffer: *mut c_void,
    _parent: *mut c_void,
    _previous: *mut c_void,
    _doc_handle: i32,
    _id: i32,
    _is_block: i32,
) -> *mut c_void {
    unimplemented!("vbuf_buffer_add_control_field_node test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_add_text_field_node(
    _buffer: *mut c_void,
    _parent: *mut c_void,
    _previous: *mut c_void,
    _text_ptr: *const u16,
    _text_len: usize,
) -> *mut c_void {
    unimplemented!("vbuf_buffer_add_text_field_node test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_add_reference_node(
    _buffer: *mut c_void,
    _parent: *mut c_void,
    _previous: *mut c_void,
    _node: *mut c_void,
) -> *mut c_void {
    unimplemented!("vbuf_buffer_add_reference_node test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_get_control_field_node_with_identifier(
    _buffer: *mut c_void,
    _doc_handle: i32,
    _id: i32,
) -> *mut c_void {
    unimplemented!(
        "vbuf_buffer_get_control_field_node_with_identifier test stub"
    );
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_is_descendant_node(
    _buffer: *mut c_void,
    _parent: *mut c_void,
    _descendant: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_buffer_is_descendant_node test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_buffer_is_node_in_buffer(
    _buffer: *mut c_void,
    _node: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_buffer_is_node_in_buffer test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_add_attribute(
    _node: *mut c_void,
    _name_ptr: *const u16,
    _name_len: usize,
    _value_ptr: *const u16,
    _value_len: usize,
) -> i32 {
    unimplemented!("vbuf_node_add_attribute test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_get_attribute(
    _node: *mut c_void,
    _name_ptr: *const u16,
    _name_len: usize,
    _ctx: *mut c_void,
    _cb: VbufStringCallback,
) -> i32 {
    unimplemented!("vbuf_node_get_attribute test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_get_attributes_string(
    _node: *mut c_void,
    _ctx: *mut c_void,
    _cb: VbufStringCallback,
) {
    unimplemented!("vbuf_node_get_attributes_string test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_get_length(_node: *mut c_void) -> i32 {
    unimplemented!("vbuf_node_get_length test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_is_block(_node: *mut c_void) -> i32 {
    unimplemented!("vbuf_node_is_block test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_is_block(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!("vbuf_node_set_is_block test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_is_hidden(_node: *mut c_void) -> i32 {
    unimplemented!("vbuf_node_is_hidden test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_is_hidden(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!("vbuf_node_set_is_hidden test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_has_useful_content(
    _node: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_node_has_useful_content test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_content_matches_string(
    _node: *mut c_void,
    _str_ptr: *const u16,
    _str_len: usize,
) -> i32 {
    unimplemented!("vbuf_node_content_matches_string test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_always_rerender_descendants(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!("vbuf_node_set_always_rerender_descendants test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_always_rerender_children(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!("vbuf_node_set_always_rerender_children test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_deny_reuse_if_previous_siblings_changed(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!(
        "vbuf_node_set_deny_reuse_if_previous_siblings_changed test stub"
    );
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_node_set_requires_parent_update(
    _node: *mut c_void,
    _value: i32,
) {
    unimplemented!("vbuf_node_set_requires_parent_update test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_get_root_doc_handle(
    _backend: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_backend_get_root_doc_handle test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_get_root_id(
    _backend: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_backend_get_root_id test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_clear_buffer(_backend: *mut c_void) {
    unimplemented!("vbuf_backend_clear_buffer test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_force_update(_backend: *mut c_void) {
    unimplemented!("vbuf_backend_force_update test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_invalidate_subtree(
    _backend: *mut c_void,
    _node: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_backend_invalidate_subtree test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_reuse_existing_node(
    _backend: *mut c_void,
    _parent: *mut c_void,
    _previous: *mut c_void,
    _doc_handle: i32,
    _id: i32,
) -> *mut c_void {
    unimplemented!("vbuf_backend_reuse_existing_node test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_pending_invalid_subtrees_empty(
    _backend: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_backend_pending_invalid_subtrees_empty test stub");
}
