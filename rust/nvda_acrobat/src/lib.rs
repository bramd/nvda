//! Rust support for the adobeAcrobat vbuf backend.
//!
//! Companion to `nvda_ia2` but for Adobe Acrobat / Reader PDFs. Where
//! gecko_ia2 renders from IAccessible2, adobeAcrobat renders from MSAA
//! `IAccessible` plus Acrobat's `IPDDom*` PDF-DOM COM interfaces
//! (`nvdaHelper/vbufBackends/adobeAcrobat/adobeAcrobat.cpp`).
//!
//! Stage 1 of the port (`docs/plans/2026-07-12-rust-vbuf-acrobat-port.md`)
//! provides only the COM interface bindings in [`interfaces`]. The
//! `fill_vbuf` render and the backend adapter land in later stages and
//! reuse the shared `nvda_vbuf::backend::run_raw_update` orchestration.

pub mod backend_state;
pub mod fill_vbuf;
pub mod interfaces;
