//! Rust support for the MSHTML (Trident) vbuf backend.
//!
//! Companion to `nvda_ia2` / `nvda_acrobat`, for the MSHTML engine used
//! by Edge IE Mode, HTML Help (`.chm`), and HTA apps. Renders from the
//! Trident DOM (`IHTMLDOMNode` / `IHTMLElement` / …) into a Rust
//! `nvda_vbuf::storage::Buffer`, driven by the shared
//! `nvda_vbuf::backend::run_raw_update` orchestration.
//!
//! Stage 1 (`docs/plans/2026-07-12-rust-vbuf-mshtml-port.md`) provides
//! only the COM interface bindings in [`interfaces`]. windows-rs 0.58 has
//! no MSHTML module, so the ~15 DOM interfaces are hand-rolled with
//! offsets taken from the SDK `MsHTML.h` `*Vtbl` structs.

pub mod backend_state;
pub mod fill_vbuf;
pub mod interfaces;
pub mod live_region;

/// Stub for the `nvdaControllerInternal_reportLiveRegion` extern, linked
/// only into test binaries (this crate's own tests, or downstream crates
/// enabling `test_stubs`). Excluded from the shipped staticlib, which
/// resolves the real symbol in `nvdaHelperRemote.dll`.
#[cfg(any(test, feature = "test_stubs"))]
mod test_stubs;
