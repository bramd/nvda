//! Stub definition of the `nvdaControllerInternal_reportLiveRegion`
//! extern declared in [`crate::live_region`]. Compiled only for this
//! crate's own tests (`cfg(test)`) and for downstream crates that enable
//! the `test_stubs` feature — mirroring `nvda_vbuf`'s `test_stubs`.
//!
//! The real symbol lives in `nvdaControllerInternal_C.obj` and is only
//! present when the aggregate staticlib is linked into
//! `nvdaHelperRemote.dll`; a test executable has no such object, so the
//! linker needs this definition to resolve the reference. It panics so
//! any test that actually reaches a live-region announcement fails loudly.

#![allow(clippy::missing_safety_doc)]

/// Matches the `extern "system"` declaration in [`crate::live_region`].
#[no_mangle]
pub unsafe extern "system" fn nvdaControllerInternal_reportLiveRegion(
    _text: *const u16,
    _level: *const u16,
) -> u32 {
    unimplemented!("nvdaControllerInternal_reportLiveRegion test stub");
}
