# Retiring the C++ vbuf storage (`vbufBase/storage.cpp`)

**Status:** DONE + SMOKE-TESTED OK (2026-07-14). All three phases
implemented and committed (phase 1 `6233e898d`, phase 2 `23f227a36`,
phase 3 `6ce934ed2`); browse-mode reads confirmed working after the sever. `vbufBase/storage.cpp` + `storage.h` are deleted; the C++
`VBufStorage_*` classes no longer exist and the DLL links no storage
symbols. ~2,150 C++ lines removed. The one behavioural change (severing the
`VBufBackend_t : VBufStorage_buffer_t` inheritance) is behaviour-preserving
because that C++ buffer was always empty for a flipped backend — but it
warrants a browse-mode smoke test of gecko / acrobat / mshtml / webKit.

## The headline

**The runtime is already 100% off C++ storage.** All five backends render
into and read out of the Rust `nvda_vbuf::storage::Buffer`; every read RPC
in `vbufRemote` already routes through the Rust ABI because every backend's
`getRustStorageBuffer()` returns non-null. Deleting `storage.cpp` is blocked
**only by compile/link-time dependencies**, not by any live behaviour. The
one genuinely behavioural change is severing the base-class inheritance and
re-homing a single `clearBuffer()` call; everything else is mechanical
dead-code removal.

This is the "Option B" end-state the earlier next-phase doc
(`2026-07-12-rust-vbuf-next-phase.md`) deferred until every backend was on
Rust storage — a precondition now met.

## What stays C++ (permanent, by design)

`backend.cpp`'s **render-thread machinery** — `SetTimer`/`requestUpdate`,
the `WH_CALLWNDPROC` + WinEvent destroy hooks, `execInThread`,
`runningBackends`, the `LockableObject`, `initialize`/`terminate`/
`forceUpdate`/`timerProc`. Rust decides *what*/*where*; C++ owns *when* and
thread affinity. Each backend's thin C++ adapter also stays (it IS the
polymorphic render-thread shim). None of that touches storage.

## Survey findings (what references the C++ storage classes)

Only these files under `nvdaHelper/` reference `VBufStorage_*`:

| File | Storage usage | Reachable at runtime? |
|---|---|---|
| `vbufBase/storage.{cpp,h}` | the definitions (1246 + 696 lines) | — |
| `vbufBase/backend.cpp/.h` | see below | one live call (`clearBuffer`) |
| `remote/vbufRemote.cpp` | 12 dead `else` branches (~45 lines) | **no** — all backends Rust |
| `vbufBase/c_shim.cpp` | ~18 storage bridges | **no** (only `clear_buffer` live) |
| `vbufBase/utils.{cpp,h}` | `nodeHasUsefulContent`, `nodeContentMatchesString` | **no** (only dead shims call them) |

`backend.cpp` specifics:

* **`VBufBackend_t : public VBufStorage_buffer_t`** (`backend.h:31`) — the
  structural root; the backend object *is-a* C++ buffer.
* **Live storage:** exactly one call — `this->clearBuffer()` in
  `renderThread_terminate` (`backend.cpp:136`), on a buffer that is always
  empty for a flipped backend. Plus the two
  `VBufStorage_controlFieldNodeList_t` members (`backend.h:60,65`), which are
  now also dead (the pending list lives in the Rust buffer; the C++
  `pendingInvalidSubtreesEmpty` shim has no live Rust caller —
  `gecko_backend_state.rs:401` queries the *Rust* buffer).
* **Dead but compiled:** base `update()` (`backend.cpp:188` — the C++
  drain/render/merge, incl. the only `new VBufStorage_buffer_t()`),
  `invalidateSubtree` (`151`), `reuseExistingNodeInRender` (`258`),
  `markNodeAsNonreusableIfInAncestor` (`140`). All overridden/bypassed.
* The pure-virtual `render(VBufStorage_buffer_t*, …)` (`backend.h:100`) +
  its 5 empty stub overrides are the last storage-type references outside
  storage.h.

## Plan — three phases, build + smoke-test each

### Phase 1 — Dead-code removal (behaviour-preserving; unreachable code)

Zero runtime risk — everything deleted here is already never executed.

* `vbufRemote.cpp`: drop the 12 dead C++-storage `else` branches so each
  read RPC routes to Rust unconditionally.
* `c_shim.cpp`: delete the ~18 dead storage bridges (`vbuf_buffer_*`,
  `vbuf_node_*`, `vbuf_backend_invalidate_subtree`,
  `vbuf_backend_reuse_existing_node`, the two `vbuf_node_*content*`). Keep
  the live `vbuf_backend_{get_root_doc_handle,get_root_id,force_update,
  request_update,get_rust_storage_buffer,clear_buffer}`.
* `utils.{cpp,h}`: delete `nodeHasUsefulContent` + `nodeContentMatchesString`.
* `backend.{cpp,h}`: delete base `update()` body (make it pure virtual),
  `invalidateSubtree`, `reuseExistingNodeInRender`,
  `markNodeAsNonreusableIfInAncestor`, the two list members, and
  `pendingInvalidSubtreesEmpty()`.
* Rust (optional): drop the now-unused `VbufBackend::
  pending_invalid_subtrees_empty` wrapper + its extern decl.

### Phase 2 — Sever the inheritance (the one behavioural change)

* `backend.h:31`: `class VBufBackend_t : public VBufStorage_buffer_t` →
  `class VBufBackend_t`. Drop `#include "storage.h"`.
* Remove the `this->clearBuffer()` call from base `renderThread_terminate`
  (each flipped backend already clears its own Rust buffer there).
* Delete the pure-virtual `render()` from the base **and** the 5 backend
  stub overrides (`render()` is fully dead once base `update()` is gone).
  This removes the final storage-type references from the backend headers.
* `c_shim.cpp`: re-home or delete `vbuf_backend_clear_buffer` (verify its
  Rust caller first; if live, route it through `getRustStorageBuffer()`
  instead of the C++ base).

Behaviour-preserving: the C++ buffer subobject being removed was always
empty for every flipped backend.

### Phase 3 — Delete `storage.cpp`/`storage.h`

* Delete `vbufBase/storage.cpp` (1246) + `vbufBase/storage.h` (696).
* Remove `storage.cpp` from `vbufBase/sconscript`.
* Drop the remaining `#include`s of `storage.h` (`backend.cpp`,
  `c_shim.cpp`, `utils.h`, `gecko_ia2.cpp`, `mshtml.h`).
* Build x64 + x86; full smoke re-test (gecko / acrobat / mshtml / webKit)
  since this removes the storage ABI type entirely.

## Impact & risk

* **~2,200+ C++ lines removed** (storage.cpp 1246 + storage.h 696 + ~45
  vbufRemote + ~130 c_shim + ~30 utils + ~120 backend.cpp), leaving a lean
  render-thread machinery + the live backend bridges + Rust-only vbufRemote.
* **Runtime risk: low.** All deleted code is unreachable or operates on an
  always-empty buffer; the `/WX` build + smoke tests are the safety net and
  catch any missed compile-time reference.
* **lotus caveat unchanged:** lotusNotesRichText is still unverified, but
  this cleanup does not alter its (already-Rust) runtime path — it only
  removes C++ storage no backend uses at runtime.
* Effort: ~1–2 sessions; mostly mechanical. Each phase is independently
  committable and revertible.
