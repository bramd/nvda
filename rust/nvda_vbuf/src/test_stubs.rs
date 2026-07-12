//! Stub definitions of every `vbuf_backend_*` extern "C" symbol
//! declared in `lib.rs`. Compiled only when the `test_stubs` feature is
//! active — see `Cargo.toml` for the rationale.
//!
//! Each stub panics so that any test that *actually* exercises a vbuf
//! backend call fails loudly. Tests in downstream crates that don't
//! touch these at all just need them to satisfy the linker.
//!
//! The storage tree itself lives in the Rust `storage::Buffer`, so the
//! former `vbuf_buffer_*` / `vbuf_node_*` storage-op externs no longer
//! exist and need no stubs; only the render-thread-machinery accessors
//! remain routed to the C-shim.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

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
pub unsafe extern "C" fn vbuf_backend_pending_invalid_subtrees_empty(
    _backend: *mut c_void,
) -> i32 {
    unimplemented!("vbuf_backend_pending_invalid_subtrees_empty test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_request_update(_backend: *mut c_void) {
    unimplemented!("vbuf_backend_request_update test stub");
}

#[no_mangle]
pub unsafe extern "C" fn vbuf_backend_get_rust_storage_buffer(
    _backend: *mut c_void,
) -> *mut c_void {
    unimplemented!("vbuf_backend_get_rust_storage_buffer test stub");
}
