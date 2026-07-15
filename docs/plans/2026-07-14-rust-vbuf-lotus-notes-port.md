# Porting the lotusNotesRichText vbuf backend to Rust

**Status:** implemented (2026-07-14), **UNVERIFIED** — no HCL/IBM/Lotus
Notes install was available to smoke-test. Fifth and final legacy backend,
after gecko_ia2, adobeAcrobat, mshtml, and webKit. The smallest port
(C++ `lotusNotesRichText.cpp` was 200 lines). With this, every vbuf
backend renders into and reads out of the Rust `storage::Buffer`.

## Why it's the simplest backend

It renders the Notes rich-text control from **plain MSAA `IAccessible`
only** — no IAccessible2, no engine-specific DOM interfaces. So:

* **No hand-rolled COM bindings** and **no `interfaces.rs`** (windows-rs
  already provides `IAccessible`).
* Node identity is the **MSAA child ID directly** (no `get_uniqueID`).
* The tree is a flat **two levels**: a synthetic client root
  (role `10` = `ROLE_SYSTEM_CLIENT`), then one control-plus-text node per
  child. **No recursion** — every child is a simple `VT_I4` child of the
  single client `IAccessible`, addressed by child ID.

## Where it lives: its own crate `nvda_lotus_notes`

Unlike webKit (which lives inside `nvda_ia2` because it needs the
`IAccessible2` binding and a separate crate would cycle), lotusNotes needs
**no `nvda_ia2` types**, so it's a **standalone crate** like acrobat /
mshtml. Its C-ABI symbols (`nvda_lotus_notes_backend_*`) ride inside the
aggregate `nvda_ia2.lib` via `extern crate nvda_lotus_notes as _;` in
`nvda_ia2/src/lib.rs` (so `nvda_vbuf`'s `#[no_mangle]` symbols aren't
duplicated across archives).

## What's reused / the shape

* `nvda_vbuf::backend::run_raw_update` + the vbufRemote read routing via
  `getRustStorageBuffer()`.
* The thin-C++-adapter + flip shape (override `update()` /
  `getRustStorageBuffer()`, gut `render()` to a stub, route the WinEvent
  hook's event filter + per-backend dispatch to Rust). Identical to the
  webKit adapter minus the id sign-flip: Notes uses child IDs as-is, both
  in the buffer and in the WinEvents it fires.

The render closure reproduces C++ `render`'s two modes: `id == 0` rebuilds
the whole tree (client root + children); a nonzero `id` re-renders that
single child's subtree into `run_raw_update`'s temp buffer, which is then
grafted back. The client `IAccessible` is resolved per render from the
document window via `WM_GETOBJECT` + `ObjectFromLresult` (a
`SendMessageTimeout`, `SMTO_ABORTIFHUNG`, 2 s — matching the C++).

## Files

Rust — new crate `rust/nvda_lotus_notes/`:

* `Cargo.toml` — staticlib+rlib; deps `nvda_vbuf` + windows/windows-core.
* `src/lib.rs` — module glue + the unverified-status note.
* `src/fill_vbuf.rs` — `resolve_client_iaccessible` (WM_GETOBJECT),
  `render_root` (client node + child enumeration), `render_control_content`
  (per-child role/state/content node).
* `src/backend_state.rs` — `LotusNotesBackendState { buffer }` + the C-ABI
  entry points (`create` / `destroy` / `get_buffer` / `clear_buffer` /
  `update`) and the hook's `win_event_is_relevant` /
  `dispatch_win_event`.

Wiring: `rust/Cargo.toml` workspace member, `nvda_ia2` path-dep +
`extern crate`, `nvdaHelper/remote/sconscript` change-detection glob.

C++ (`nvdaHelper/vbufBackends/lotusNotesRichText/`):

* `lotusNotesRichText.cpp` (200 → ~115) — flipped to a thin adapter;
  `render()` gutted to a stub, dead `renderControlContent` + the root
  enumeration removed.
* `lotusNotesRichText.h` — adds `rustState`, `update()`,
  `getRustStorageBuffer()`, destructor; removes the `renderControlContent`
  decl.

## Testing

**Needs a real Notes install to verify.** The backend attaches to the HCL
Notes (formerly Lotus/IBM Notes) rich-text editor control. Someone with
Notes should browse a rich-text document/field with NVDA-from-source and
confirm the virtual buffer renders text and updates on edits. x64 and x86
`nvdaHelperRemote.dll` are both built and link clean `/WX`; the Rust
compiles and the existing gecko/acrobat/mshtml/webKit test suites are
unaffected — but the render path itself has had **no live exercise**.
