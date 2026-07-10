//! Shared `BSTR` helpers.

use windows::core::BSTR;

/// Probe whether a `BSTR` is NULL as opposed to zero-length.
///
/// `BSTR::is_empty()` returns `true` for *both* a NULL `BSTR` and a
/// non-NULL zero-length one, but several COM helpers need to tell the
/// two apart: a NULL `BSTR` means "the server returned nothing usable"
/// while a zero-length `BSTR` is a successful empty string.
///
/// SAFETY: `windows::core::BSTR` is `#[repr(transparent)]` over a single
/// `*const u16` field (verified in `windows-strings-0.1.0/src/bstr.rs:6`),
/// so reinterpreting a `&BSTR` as a `*const *const u16` and reading it
/// yields that pointer.
pub(crate) fn is_bstr_null(bstr: &BSTR) -> bool {
    let raw_ptr: *const u16 =
        unsafe { *(bstr as *const _ as *const *const u16) };
    raw_ptr.is_null()
}
