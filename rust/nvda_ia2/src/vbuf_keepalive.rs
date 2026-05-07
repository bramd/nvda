//! Phase 6d-b plumbing: keep `nvda_vbuf_*` extern API symbols alive
//! through the staticlib chain.
//!
//! `nvda_vbuf` is built as an `rlib` and consumed by this crate as a
//! cargo dep. The `nvda_vbuf_*` symbols defined in
//! `nvda_vbuf::extern_api` have no callers in `nvda_ia2` yet (they
//! are intended for C++ callers in Phase 6e once `gecko_ia2` owns a
//! Rust `Buffer`), so Rust's dead-code elimination strips them out
//! before they reach `nvda_ia2.lib`. That in turn means they never
//! appear in `nvdaHelperRemote.dll`.
//!
//! This module references every `nvda_vbuf_*` function through a
//! `#[no_mangle]` C-callable address-table getter. The function is
//! never expected to be called in production -- its sole purpose is
//! to give the linker a chain of references that keeps each
//! `nvda_vbuf_*` symbol from being elided.
//!
//! ## When to delete this module
//!
//! Phase 6e will introduce real C++ call sites for the
//! `nvda_vbuf_*` functions. Once at least one caller per function
//! exists, this keepalive becomes redundant -- delete it then.

use core::ffi::c_void;

use nvda_vbuf::extern_api;

/// One entry per `nvda_vbuf_*` function in the parallel extern API.
/// The order is documentation-only -- callers should not rely on it.
#[allow(clippy::type_complexity)]
const NVDA_VBUF_EXTERN_API_ADDRS: &[*const ()] = &[
    extern_api::nvda_vbuf_buffer_create as *const (),
    extern_api::nvda_vbuf_buffer_destroy as *const (),
    extern_api::nvda_vbuf_buffer_clear as *const (),
    extern_api::nvda_vbuf_buffer_text_length as *const (),
    extern_api::nvda_vbuf_buffer_has_content as *const (),
    extern_api::nvda_vbuf_buffer_add_control_field_node as *const (),
    extern_api::nvda_vbuf_buffer_add_text_field_node as *const (),
    extern_api::nvda_vbuf_buffer_add_reference_node as *const (),
    extern_api::nvda_vbuf_buffer_get_control_field_node_with_identifier
        as *const (),
    extern_api::nvda_vbuf_buffer_is_descendant_node as *const (),
    extern_api::nvda_vbuf_buffer_is_node_in_buffer as *const (),
    extern_api::nvda_vbuf_buffer_locate_control_field_node_at_offset
        as *const (),
    extern_api::nvda_vbuf_buffer_field_node_offsets as *const (),
    extern_api::nvda_vbuf_buffer_is_field_node_at_offset as *const (),
    extern_api::nvda_vbuf_node_identifier as *const (),
    extern_api::nvda_vbuf_buffer_get_text_in_range as *const (),
    extern_api::nvda_vbuf_buffer_get_selection_offsets as *const (),
    extern_api::nvda_vbuf_buffer_set_selection_offsets as *const (),
    extern_api::nvda_vbuf_buffer_line_offsets as *const (),
    extern_api::nvda_vbuf_node_add_attribute as *const (),
    extern_api::nvda_vbuf_node_get_attribute as *const (),
    extern_api::nvda_vbuf_node_get_attributes_string as *const (),
    extern_api::nvda_vbuf_node_get_length as *const (),
    extern_api::nvda_vbuf_node_is_block as *const (),
    extern_api::nvda_vbuf_node_set_is_block as *const (),
    extern_api::nvda_vbuf_node_is_hidden as *const (),
    extern_api::nvda_vbuf_node_set_is_hidden as *const (),
    extern_api::nvda_vbuf_node_has_useful_content as *const (),
    extern_api::nvda_vbuf_node_content_matches_string as *const (),
    extern_api::nvda_vbuf_node_set_always_rerender_descendants as *const (),
    extern_api::nvda_vbuf_node_set_always_rerender_children as *const (),
    extern_api::nvda_vbuf_node_set_deny_reuse_if_previous_siblings_changed
        as *const (),
    extern_api::nvda_vbuf_node_set_requires_parent_update as *const (),
    extern_api::nvda_vbuf_buffer_invalidate_subtree as *const (),
    extern_api::nvda_vbuf_buffer_pending_invalid_subtrees_empty as *const (),
    extern_api::nvda_vbuf_buffer_reuse_existing_node as *const (),
];

/// Address-table getter referenced through a `#[no_mangle]` symbol so
/// the linker can't elide it, which transitively keeps every entry of
/// [`NVDA_VBUF_EXTERN_API_ADDRS`] alive.
///
/// Production code does not call this. It exists purely to defeat
/// dead-code elimination during the staticlib build. The signature is
/// shaped so a future test (or a casual inspection from C/C++) can
/// dump the table: pass a non-null pointer to receive the count, and
/// the function returns the table's base address. Pass null to ignore.
///
/// # Safety
///
/// `out_count` must be either null or point to a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvda_ia2_nvda_vbuf_extern_api_addrs(
    out_count: *mut usize,
) -> *const *const c_void {
    let table = NVDA_VBUF_EXTERN_API_ADDRS;
    if !out_count.is_null() {
        unsafe { *out_count = table.len() };
    }
    table.as_ptr() as *const *const c_void
}

#[cfg(test)]
mod tests {
    use super::*;
    use nvda_vbuf::storage::Buffer;

    #[test]
    fn keepalive_table_has_every_extern_api_function() {
        // Sanity check: bumping NVDA_VBUF_EXTERN_API_ADDRS is the
        // hook that catches "you added a new nvda_vbuf_* function and
        // forgot the keepalive entry".
        assert_eq!(NVDA_VBUF_EXTERN_API_ADDRS.len(), 36);
        for (i, &p) in NVDA_VBUF_EXTERN_API_ADDRS.iter().enumerate() {
            assert!(!p.is_null(), "entry {} is null", i);
        }
    }

    #[test]
    fn nvda_ia2_nvda_vbuf_extern_api_addrs_is_callable() {
        let mut count: usize = 0;
        let ptr = unsafe {
            nvda_ia2_nvda_vbuf_extern_api_addrs(&mut count)
        };
        assert!(!ptr.is_null());
        assert_eq!(count, 36);
    }

    /// Smoke-test that the table actually points at functioning
    /// `nvda_vbuf_*` entry points. Calls `buffer_create` /
    /// `buffer_destroy` (entries 0 and 1) through the table.
    #[test]
    fn create_destroy_through_table_round_trips() {
        type Create = unsafe extern "C" fn() -> *mut Buffer;
        type Destroy = unsafe extern "C" fn(*mut Buffer);
        unsafe {
            let create: Create =
                core::mem::transmute(NVDA_VBUF_EXTERN_API_ADDRS[0]);
            let destroy: Destroy =
                core::mem::transmute(NVDA_VBUF_EXTERN_API_ADDRS[1]);
            let b = create();
            assert!(!b.is_null());
            destroy(b);
        }
    }
}
