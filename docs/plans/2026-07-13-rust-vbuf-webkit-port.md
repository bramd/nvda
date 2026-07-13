# Porting the WebKit vbuf backend to Rust

**Status:** implemented (2026-07-13), pending manual smoke test. Fourth
backend after gecko_ia2, adobeAcrobat, and mshtml. The smallest port yet
(C++ `webKit.cpp` was 228 lines). Follows the proven per-backend pattern;
the shared `nvda_vbuf::backend::run_raw_update` orchestration is reused.

## Why WebKit is the easiest port

WebKit is a much simpler cousin of gecko_ia2: both are **IAccessible2 +
MSAA** backends driven by a **WinEvent hook** (not a COM change sink).
WebKit uses only a single IA2-specific call, `get_uniqueID`, for node
identity — everything else (`accRole`, `accState`, `accChildCount`,
`accName`/`accValue`/`accDescription`, `AccessibleChildren`) is plain
MSAA `IAccessible`, already in windows-rs. So there are **no new COM
bindings** to hand-roll and **no `interfaces.rs`** — it reuses
`nvda_ia2::interfaces::IAccessible2`.

## Where it lives: modules inside `nvda_ia2`, not a new crate

Unlike acrobat/mshtml (own crates), WebKit is **modules inside
`nvda_ia2`**. A separate `nvda_webkit` crate would need `nvda_ia2`'s
`IAccessible2` binding, but `nvda_ia2` must force-link every backend into
its aggregate `nvda_ia2.lib` — that would be a **dependency cycle**.
`nvda_ia2` is "IA2-based backends," not "gecko-only," so WebKit belongs
there. Bonus: its C-ABI symbols land in the aggregate automatically —
no `extern crate ... as _` force-link needed.

## What's reused (proven on gecko)

- `nvda_ia2::interfaces::IAccessible2`, `nvda_ia2::from_identifier`
  (WebKit's `IAccessible2FromIdentifier` is gecko's with the id sign
  flipped — WebKit stores positive unique IDs but queries/fires events
  with negative ones).
- `nvda_vbuf::storage::Buffer`, `run_raw_update`, and the vbufRemote read
  routing via `getRustStorageBuffer()`.
- The thin-C++-adapter + flip shape: override `update()` /
  `getRustStorageBuffer()`, gut `render()` to an empty pure-virtual
  satisfier, keep the WinEvent-hook machinery in C++ but route its
  event filter + per-backend dispatch to Rust.

## Simpler than gecko in every axis

- No toolkit-name / `is_chrome` handling.
- No cached root accessible, no `IA2_STATE_DEFUNCT` liveness check.
- **No cross-render node reuse.** The C++ backend's `render()` ignored
  `oldNode` and re-rendered each invalidated subtree fresh; the Rust
  render closure does the same (ignores `main` and `old_node`), and
  `run_raw_update`'s re-render path renders fresh into a temp buffer +
  swaps it in — reuse-in-render is an optional optimization WebKit never
  had.
- A single constant `docHandle` (the root document window) threaded
  unchanged through the whole recursion — WebKit never derives a per-node
  docHandle the way gecko does.
- The WinEvent hook reacts to only three events
  (VALUECHANGE / STATECHANGE / REORDER) and has no force-update or
  root-state-change special cases.

## Files

Rust (in `rust/nvda_ia2/src/`):

- `webkit_fill_vbuf.rs` — the `fillVBuf` port (MSAA role/state/name walk).
- `webkit_backend_state.rs` — `WebKitBackendState { buffer }` +
  C-ABI entry points: `create` / `destroy` / `get_buffer` /
  `clear_buffer` / `update`, and the hook's `win_event_is_relevant` +
  `dispatch_win_event`. Homes the WebKit-specific id-negating
  `from_identifier` wrapper.
- `lib.rs` — registers the two modules.

C++ (`nvdaHelper/vbufBackends/webKit/`):

- `webKit.cpp` (228 → ~115) — flipped: `extern "C"` block for the Rust
  entry points, WinEvent hook routes to Rust, `update()` /
  `getRustStorageBuffer()` overrides, ctor/dtor manage `rustState`,
  `render()` gutted to a stub. Dead C++ `fillVBuf` +
  `IAccessible2FromIdentifier` removed.
- `webKit.h` (44 → ~70) — adds `rustState`, `update()`,
  `getRustStorageBuffer()`, destructor; removes the `fillVBuf` decl.

## Testing on Windows 11

WebKit-on-Windows is rare now, but NVDA's WebKit backend still attaches
to **iTunes for Windows** (its Store views render via an embedded WebKit
control) — the `itunes` app module maps to `virtualBuffers/webKit.py`.
iTunes (classic) is 32-bit, so both the x64 and x86
`nvdaHelperRemote.dll` are rebuilt. Smoke test: browse an iTunes Store
page with NVDA from source and confirm the virtual buffer renders +
updates.
