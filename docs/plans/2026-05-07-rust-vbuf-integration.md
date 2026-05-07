# Wiring the Rust vbuf storage into the live code path

**Status:** design (2026-05-07). No code yet.

Phase 6b/6c built a Rust port of `vbufBase`'s storage and backend
orchestration in `rust/nvda_vbuf/`. 69 tests cover every surface
(buffer / nodes / navigation / markup / replace_subtrees / Renderer
trait / update). **Nothing live uses any of it yet.** This document
maps the path from "isolated Rust crate" to "Rust storage backs the
live gecko_ia2 vbuf rendering."

## Recap of where we are

The C-shim `nvdaHelper/vbufBase/c_shim.cpp` exposes 27 `extern "C"`
functions over the C++ `VBufStorage_buffer_t` / `VBufBackend_t` /
`VBufStorage_fieldNode_t` classes. The Rust bindings in
`rust/nvda_vbuf/src/lib.rs` declare those externs and wrap them in
opaque-handle newtypes. `nvda_ia2`'s fill_vbuf calls these wrappers,
so every vbuf operation crosses an FFI boundary and ends up in C++.

The Rust Buffer in `rust/nvda_vbuf/src/storage/` is structurally
equivalent and tested in isolation, but no extern "C" API exposes
it, and no caller knows about it.

## The core question: how invasive a migration?

There are three plausible shapes; each commits us to a different
amount of disruption.

### Shape A — Parallel API, gecko_ia2 first

Add a brand-new `extern "C"` API that operates on `Box<Buffer>`:

```c
void* nvda_vbuf_buffer_create();
void  nvda_vbuf_buffer_destroy(void* buffer);
void* nvda_vbuf_buffer_add_control_field_node(void* buffer, ...);
// ... mirror every existing vbuf_* function ...
```

`nvda_ia2`'s fill_vbuf swaps its existing nvda_vbuf wrappers for
direct calls into the Rust Buffer (no extern hop -- it just becomes
`buffer.add_control_field_node(...)` on a `&mut Buffer`).

Other backends (mshtml / webKit / lotusNotesRichText / adobeAcrobat)
keep using the existing C-shim with C++ storage. Both subsystems
coexist.

**Cost:** ~1 day for the parallel API, ~2 days for the gecko_ia2
migration. The other backends are untouched -- defer their migration
indefinitely.

**Risk:** medium. Touches gecko_ia2's render path. If the Rust
storage has bugs not caught by unit tests, regressions surface only
when browsing real pages.

**Drawback:** until other backends migrate, vbufBase has *two*
storage implementations. Some maintenance overhead.

### Shape B — Flip the C-shim's underlying storage

Same `extern "C"` function names. Same callers. But the
implementation moves: `c_shim.cpp` is deleted, replaced by
`rust/nvda_vbuf/src/c_shim.rs`. The `void*` opaque pointer changes
identity from "C++ class instance" to "Rust `Box<Buffer>` raw
pointer."

Every backend's `*_createInstance` function allocates a Rust Buffer.
The C++ `VBufStorage_buffer_t` and `VBufBackend_t` classes either
disappear (their state is now Rust-owned) or shrink to a thin
adapter shell.

**Cost:** ~3-5 days for the flip, plus per-backend adapter work.

**Risk:** high. Big-bang. Every backend changes simultaneously.
Regressions potentially affect every browser, every Office app,
every PDF viewer NVDA supports through these backends.

**Drawback:** mshtml / webKit / lotusNotesRichText / adobeAcrobat
inherit from `VBufBackend_t` and use protected members
(`pendingInvalidSubtreesList`, etc.). The C++ adapter shell needs
to expose those somehow, or those backends need rewriting.

### Shape C — Validation-only first

Don't touch any live code. Add a Rust function that, given a C++
buffer pointer, walks the tree (via the existing C-shim) and
constructs an equivalent Rust Buffer. Compare them on every
operation: does the Rust port produce the same `getTextInRange`
output as the C++? Same markup? Same line offsets?

Run NVDA against the C++ path; in the background, build a
shadow Rust buffer; on every read, compare and log mismatches.

**Cost:** ~2 days for the shadow infrastructure plus running it
against pages.

**Risk:** zero -- doesn't affect live behavior.

**Drawback:** doesn't actually integrate anything. Just builds
confidence. Could surface bugs in the Rust port before we commit
to a migration.

## Recommendation: Shape A (parallel API + gecko_ia2 first)

Reasoning:

* **Smallest blast radius.** Only gecko_ia2 changes. Firefox and
  Chrome users get a Rust storage; everyone else (Office, PDF,
  Lotus Notes, etc.) is untouched.
* **Real validation.** Real browsing exercises the storage in
  ways no unit test can. Bugs that exist surface fast in a
  smoke-test scenario the user is already comfortable with.
* **Reversible.** If something breaks, the C-shim path still
  exists; we can route gecko_ia2 back through it by reverting
  the wiring change.
* **Builds toward the long-term goal** without committing to it.
  Once gecko_ia2 is comfortably on Rust storage, the same pattern
  applies to the next backend (or we keep the parallel APIs
  indefinitely if other backends never migrate).

Shape B is the eventual end-state but doesn't have to be done in
one step. Shape C is academically interesting but has a poor
work-to-progress ratio.

## Phased plan for Shape A

Each phase is a coherent commit chain that leaves the tree green.

### Phase 6d-a -- the parallel `nvda_vbuf` extern API (~1 day)

Add `rust/nvda_vbuf/src/extern_api.rs`. Each function takes the
opaque `Box<Buffer>` pointer (created via
`nvda_vbuf_buffer_create`) and forwards to the Rust `Buffer`
methods. The function names are deliberately distinct from the
existing C-shim (`nvda_vbuf_*` vs `vbuf_*`) so both APIs can
coexist.

Surface (mirrors the existing C-shim 1:1):

* Buffer lifecycle: `create`, `destroy`, `clear`, `text_length`.
* Node insertion: `add_control_field_node`, `add_text_field_node`,
  `add_reference_node`.
* Tree queries: `get_control_field_node_with_identifier`,
  `is_descendant_node`, `is_node_in_buffer`, `field_node_offsets`,
  `is_field_node_at_offset`, `has_content`.
* Text retrieval: `get_text_in_range` (with markup flag).
* Selection: `get_selection_offsets`, `set_selection_offsets`,
  `line_offsets`.
* Node attrs: `add_attribute`, `get_attribute`,
  `get_attributes_string`, `get_length`, `is_block`,
  `set_is_block`, `is_hidden`, `set_is_hidden`,
  `has_useful_content`, `content_matches_string`.
* Control field flags: `set_always_rerender_descendants` /
  `_children`, `set_deny_reuse_if_previous_siblings_changed`,
  `set_requires_parent_update`.
* Backend-side: `invalidate_subtree`, `replace_subtrees`,
  `force_update`, `reuse_existing_node`,
  `pending_invalid_subtrees_empty`.

The string OUT-callback type matches the existing C-shim's
`vbuf_string_callback` for consistency.

### Phase 6d-b -- swap nvda_vbuf's bindings for direct Rust calls (~1 day)

Currently `rust/nvda_vbuf/src/lib.rs` declares `extern "C"`
functions whose definitions live in `c_shim.cpp`. The opaque
newtypes (`VbufBuffer`, `VbufFieldNode`, etc.) wrap these calls.

Add a feature flag `direct_rust_storage` (default off). When
enabled, the newtype wrappers route to the Rust `Buffer` methods
directly instead of crossing the FFI boundary. When disabled,
behavior is unchanged.

This step is internal to nvda_vbuf — no callers see any
difference yet. Lets us A/B test.

### Phase 6e — gecko_ia2 owns its Rust Buffer (~2 days)

Add a `Buffer` field to `nvda_ia2::gecko_backend_state::GeckoBackendState`.
Change `nvda_ia2_fill_vbuf` to take that Buffer instead of the
existing `void* buffer` (which was a C++ pointer).

The C++ `GeckoVBufBackend_t::render` shim today calls
`nvda_ia2_gecko_backend_render(state, this, ...)` where `this`
is the C++ buffer. Change the signature so render takes the Rust
Buffer pointer instead. The C++ side either:

1. Stops using its inherited C++ `VBufStorage_buffer_t` storage
   for gecko_ia2 specifically, and routes all backend method calls
   (clearBuffer, invalidateSubtree, etc.) through the Rust Buffer.
2. Maintains both, with the C++ as a "shadow" that's never read by
   anyone — wasteful but minimally invasive.

Option 1 is the right end-state. Each `VBufBackend_t` virtual
method on `GeckoVBufBackend_t` gets overridden to delegate to the
Rust Buffer.

The Python side calls into `VBufBackend_t::getTextInRange` and
similar through NVDA's vbuf API. Those reads go through the C++
class methods. We need each to reach the Rust storage.

This is the substantive integration commit. The first one with
real regression risk — needs a smoke test on Firefox + Chrome.

### Phase 6f — delete gecko_ia2.cpp (~1 day)

After 6e settles, the C++ `GeckoVBufBackend_t` class is a thin
adapter that holds the Rust state. The factory
`GeckoVBufBackend_t_createInstance` can move into Rust as well,
with a tiny C++ stub for the polymorphic interface NVDA expects.

This was Phase 5's stretch goal; it's revisited here once the
storage is also Rust.

## What stays C++ indefinitely

* `VBufBackend_t`'s Win32 render-thread machinery (timer, hook
  registration, `execInThread`). Already wired into NVDA's
  helper system — porting it to Rust adds unsafe FFI for
  marginal benefit.
* The 4 unported backends (mshtml / webKit / lotusNotesRichText /
  adobeAcrobat). They keep using the C-shim with C++ storage.
  If we want them on Rust storage too, that's per-backend
  follow-up work modeled on the gecko_ia2 migration.

## Open questions

* **Buffer ownership model in gecko_ia2.** Does the Rust Buffer
  live on `GeckoBackendState` (per-backend), or does the C++
  `GeckoVBufBackend_t` allocate it via the new
  `nvda_vbuf_buffer_create` extern and store the raw pointer?
  Likely the former — `GeckoBackendState` already owns the Rust
  state, and embedding the Buffer there avoids one layer of
  indirection.
* **`reuse_existing_node_in_render` boundary.** The Rust port
  takes `&mut self` (the main buffer) and `&Buffer` (the temp
  buffer). The C++ version takes raw pointers. Need to decide
  the FFI shape: pass both pointers and have the extern call
  borrow them; or hold the Rust state across the FFI in a
  thread-local.
* **Migration of `VBufBackend_t` virtuals.** `clearBuffer`,
  `invalidateSubtree`, `update`, `forceUpdate`, etc. are virtual
  on `VBufBackend_t`. `GeckoVBufBackend_t` already overrides
  some. After migration each needs to operate on the Rust
  Buffer, not the inherited C++ storage. Which methods to
  override (vs. let the inherited C++ do nothing) is a per-
  method decision driven by what the Python side calls.

## Stopping points

* **After 6d-a:** parallel API exists. Tested in isolation.
  Nothing live uses it. Easy to revert.
* **After 6d-b:** `nvda_vbuf` newtypes can route to Rust storage
  via a feature flag. Default still off. Useful for A/B testing.
* **After 6e:** gecko_ia2 uses Rust storage in production. First
  commit with real regression risk. Smoke-test gate.
* **After 6f:** gecko_ia2.cpp is gone. End state for the
  gecko_ia2 path.

Each is a clean stopping point. We can pause indefinitely at any
of them.
