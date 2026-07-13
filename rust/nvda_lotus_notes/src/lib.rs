//! Rust support for the lotusNotesRichText vbuf backend.
//!
//! The simplest and most legacy of NVDA's vbuf backends
//! (`nvdaHelper/vbufBackends/lotusNotesRichText/lotusNotesRichText.cpp`,
//! ~200 lines). It renders the HCL Notes (formerly Lotus Notes / IBM
//! Notes) rich-text control from **plain MSAA `IAccessible`** — no
//! IAccessible2, no engine-specific DOM interfaces — so it needs no
//! hand-rolled COM bindings (unlike acrobat / mshtml). Node identity is
//! the MSAA child ID directly, and the tree is a flat two levels: a
//! client root, then one control-plus-text node per child, addressed by
//! child ID off the single client `IAccessible`.
//!
//! Reuses the shared `nvda_vbuf::backend::run_raw_update` orchestration
//! and the vbufRemote read routing via `getRustStorageBuffer()`, exactly
//! like the other flipped backends.
//!
//! NOTE: this port is unverified — no Lotus Notes install was available to
//! smoke-test it. It follows the proven gecko/webKit WinEvent-hook pattern
//! and the C++ original faithfully, but must be exercised against real
//! Notes before it can be called working.

pub mod backend_state;
pub mod fill_vbuf;
