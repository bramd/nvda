# Porting the adobeAcrobat vbuf backend to Rust

**Status:** design (2026-07-12). Not started. Next backend after gecko_ia2 on
the road to a fully-Rust vbuf storage. adobeAcrobat (PDF accessibility in Adobe
Acrobat / Reader) is well-used and important; mshtml / webKit /
lotusNotesRichText are legacy and deferred.

## Why adobeAcrobat is a good second backend

* It exercises the Rust storage + orchestration built for gecko against a
  **different COM surface** (IAccessible/MSAA + Acrobat's `IPDDom*` PDF-DOM
  interfaces, not IAccessible2). Proving the storage generalises here de-risks
  the eventual retirement of C++ `storage.cpp`.
* It's **smaller than gecko**: `adobeAcrobat.cpp` is 869 lines with an ~318-line
  `fillVBuf` (gecko's was ~854), and most of the hard infrastructure already
  exists.

## What's already built (reused as-is from 6e)

* **`nvda_vbuf::storage::Buffer`** — the storage. Unchanged.
* **vbufRemote read routing is backend-agnostic.** Stage C branches every read
  RPC on `backend->getRustStorageBuffer()`. The moment adobeAcrobat's backend
  returns its embedded Rust `Buffer`, all reads route to Rust automatically —
  **no vbufRemote changes.**
* **The update/merge/reuse machinery** — `take_pending_into_working`,
  `replace_subtrees`, `reuse_existing_node_in_render`, `Node`, markup,
  `find_node_by_attributes`. Identical across backends.
* **The backend orchestration** (drain pending → render each invalid subtree
  into a temp `Buffer` → `replace_subtrees`) is identical across backends; only
  the **render function** (fillVBuf) differs.

## New work

1. **Acrobat COM bindings** — bind `IPDDomNode`, `IPDDomNodeExt`,
   `IPDDomElement`, `IPDDomDocPagination`, `IGetPDDomNode`, `IAccID` (and the
   `SID_*` service ids) from `miscDeps/include/AcrobatAccess/AcrobatAccess.idl`
   (typelib at `source/typelibs/AcrobatAccess.tlb`). Mirror the hand-rolled
   `define_interface!` vtable pattern in `rust/nvda_ia2/src/interfaces.rs`.
   Medium effort; must match vtable layout exactly (same discipline as IA2).
2. **fillVBuf → Rust** (~318 lines): role/state/name from IAccessible, language
   (fetch + parent-inherit), page number (`getPageNum` via `IPDDomNodeExt`),
   table info (explicit `Headers` attribute, col/row-span tracking), text
   rendering (`renderText`), and the AccessibleChildren walk. Follows the
   `rust/nvda_ia2/src/fill_vbuf.rs` structure with Acrobat interfaces.
3. **Backend adapter + orchestration wiring** — a per-backend state struct with
   an embedded `Buffer`, the C++ `AdobeAcrobatVBufBackend_t` reduced to a thin
   adapter (like `gecko_ia2.cpp` today), the orchestration externs, and
   `getRustStorageBuffer()`. Plus the special cases `checkIsXFA` and
   `getDocPagination`.

## Key design decisions

### Crate layout — new `rust/nvda_acrobat` crate

links into `nvdaHelperRemote.dll`. Keeps IA2 and Acrobat concerns separate.

### Extract a backend-generic orchestration first (Stage 0)

Today the gecko orchestration lives in
*ackend-agnostic except for the call to`fill_vbuf`. **Before touching Acrobat,
refactor this into a reusable form** so both backends share it and future
*ackends are "just port fillVBuf". Two shapes:
*

* **Preferred:** finish wiring `nvda_vbf::backend`'s existing `Renderer` trait +
* `update()` so a backend supplies a `Renderer` (its fillVBuf) and the shared
  `update` drives drain/render/merge. Reconcile it with the raw-pointer/borrow
* constraints that pushed gecko to hand-roll the loop (the main-vs-temp buffer

  aliasing, the `ctx.main` reuse lookup).
* **Fallback:** a shared generic fn `run_backend_update(state, backend, render_fn)`
  in a small shared module, with `render_fn` a per-backend function pointer.

Do this as a **behaviour-preserving refactor of gecko** (verify gecko still

renders full pages + the vbuf_bench/tests pass) so Acrobat plugs into a proven
path. This is the single most valuable step for the "port all backends" goal.

### The custom-node `language` field → store as an attribute, not a Node field

C++ subclasses the node (`AdobeAcrobatVBufStorage_controlFieldNode_t`) with one
`wstring language`, set from the node's Lang, inherited from the parent, and —

re-render (`oldNode->getParent()->language`). To keep `nvda_vbuf::Node` generic
(no per-backend fields), store language as the existing `"language"` **attribute**

and, during update, read that attribute off the old parent control node instead
of a dedicated field. Action item: confirm/ensure control nodes (not just text
*odes) carry the `"language"` attribute so the inheritance read works; if the

C++ only kept it in the field, add the attribute in the Rust port.
*
*## Table handling — port as threaded Rust state

*TableInfo` / `TableHeaderInfo` / `columnRowSpans` / `headersInfo`(explicit
*Headers` attribute parsing, col/row-span accounting) are self-contained; port
*s Rust structs threaded through the render (as gecko threads its table state).

*# Staged plan (each stage compiles, tests, and links green)
*

* **Stage 0 — backend-generic orchestration refactor.** Extract the shared
* update/drain/merge/reuse from gecko; gecko still uses it unchanged. Verify:
* gecko renders full pages (browse-mode smoke test), `cargo test`, `vbuf_bench`.
* **Stage 1 — Acrobat COM bindings.** New `nvda_acrobat` crate; bind the
* `IPDDom*` interfaces from the IDL. Unit-test what's testable (vtable offsets,

* simple calls against a stub).
* **Stage 2 — fillVBuf port.** The Acrobat render into a Rust `Buffer`: role/
  name/state, language, page num, tables, text, children. Bulk of the work.
* **Stage 3 — backend adapter + wiring (dormant).** `AdobeAcrobatBackendState`
* with an embedded `Buffer`; reduce `adobeAcrobat.cpp` to the thin adapter +
  `getRustStorageBuffer()` + orchestration externs; add `nvda_acrobat` to the
  scons build. Not yet flipped (getRustStorageBuffer returns the Rust buffer
  only at Stage 4).
* **Stage 4 — THE FLIP + smoke test.** Route Acrobat's render/storage to Rust.
* Gate on a smoke test in Adobe Acrobat/Reader against a tagged PDF: linear
  reading, headings/links quick-nav, a table (with row/column headers), a
* multi-page document, and an XFA form. Single behaviour-changing commit;
* `git revert` restores the C++ path.
*

## Risks

*
* **COM binding fidelity.** The `IPDDom*` vtable layouts must match the IDL

* exactly (an off-by-one vtable slot = calling the wrong method = garbage), same

  risk class as the IA2 bindings. Mitigate by deriving strictly from
* `AcrobatAccess.idl` and testing against a real PDF early.
* **`language` inheritance + table edge cases** across partial re-renders.
* **Smoke-test tooling.** Needs Adobe Acrobat or Reader installed with a
  well-tagged PDF (and ideally a table + an XFA form) to exercise the paths.
* **Traffic vs risk.** Lower traffic than Firefox/Chrome, but PDF accessibility
  is important; treat Stage 4 with the same smoke-test rigor as the gecko flip.

## Effort

Smaller than the gecko_ia2 port: fillVBuf is ~⅓ the size and all the storage /
read-routing / update machinery is reused. The two real new costs are the
Acrobat COM bindings and the Stage 0 orchestration refactor — the latter pays
forward to every remaining backend.

## After adobeAcrobat

The remaining backends (mshtml, webKit, lotusNotesRichText) are legacy; migrate
them (or leave them on C++ storage) later. C++ `storage.cpp` can only be deleted
once **all** backends are off it; `backend.cpp` (render-thread machinery) stays
C++ regardless, per the 6e design. So the realistic end-state remains "Rust
storage + a thin C++ render-thread adapter per backend," reached one backend at
a time.
