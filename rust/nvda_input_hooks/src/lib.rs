/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

//! NVDA Helper Remote: Rust port of Windows-hook-based input event handlers.
//!
//! This crate is built as a `staticlib` and linked into `nvdaHelperRemote.dll`.
//! It is loaded into target processes via DLL injection, so the same code runs
//! inside Word, Chrome, every browser tab, etc. Keep allocations minimal and do
//! not rely on host-process global state.
