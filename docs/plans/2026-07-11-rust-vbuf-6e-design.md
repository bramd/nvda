# Phase 6e: gecko_ia2 live on the Rust vbuf storage

**Status:** implemented, pending smoke test (2026-07-11). Stages A–D
landed; the flip (Stage D) rides `nvda_ia2`'s `default = ["direct_rust_storage"]`
feature. Gated on a Firefox + Chrome smoke test before the flip commit is
considered done.

Phase 6d-a/6d-b built and tested the Rust `storage::Buffer`, a parallel
`nvda_vbuf_*` C ABI over it (`extern_api.rs`), and a `direct_rust_storage`
Cargo feature that re-points the `nvda_vbuf` newtypes at the Rust
`Buffer` instead of the C-shim. Nothing live uses the Rust storage yet:
`nvda_ia2` builds with the feature **off**, so `fillVBuf` still renders
through `c_shim.cpp` into the C++ `VBufStorage_buffer_t`.

Phase 6e makes the **gecko_ia2 backend only** (Firefox / Chrome / any
IAccessible2 document) render into, and read out of, the Rust `Buffer`.
The four other backends (mshtml, webKit, adobeAcrobat,
lotusNotesRichText) are untouched and keep the C++ storage.

This document resolves the hard problems the 6d plan left open — node
identity across the C++ virtuals, locking, update/render orchestration,
the feature flip, and the reversal path — and specifies every file,
virtual, and extern the migration touches.

## The decisive fact: the RPC node handle is already 64-bit

`nvdaHelper/interfaces/vbuf/vbuf.idl:41` declares

```idl
typedef unsigned hyper VBufRemote_nodeHandle_t;
```

`unsigned hyper` is a **64-bit** MIDL type on every architecture,
including x86. So the node handle that crosses from `nvdaHelper` to the
Python `virtualBuffers` layer is *already* 64 bits wide everywhere.
Today `vbufRemote.cpp` casts a `VBufStorage_fieldNode_t*` into it (a
32-bit pointer on x86, zero-extended). A Rust `slotmap` `NodeKey` is a
`u64` (`KeyData::as_ffi`). **A key fits the RPC handle exactly, with no
narrowing, on all arches.** This is what makes 6e tractable without
per-buffer stub-node tables.

The buffer handle `VBufRemote_bufferHandle_t` is a `[context_handle]
void*` — it stays a `VBufBackend_t*` in all configs; only the *node*
handle changes meaning for gecko.

## The consumer inventory (who touches a gecko buffer)

The migration is only safe if we have enumerated every in-process
reader of a gecko backend's storage. Grepping `nvdaHelper` for
`VBufBackend_t` / `VBufStorage_buffer_t` / `getTextInRange` /
`findNodeByAttributes` / `getControlFieldNodeWithIdentifier` /
`reuseExistingNodeInRender` yields, besides the base classes
themselves and the four other backends:

* **`nvdaHelper/remote/vbufRemote.cpp`** — the *sole* external
  in-process consumer. Every `VBufRemote_*` RPC casts the buffer handle
  to `VBufBackend_t*`, acquires `backend->lock`, and calls a storage
  virtual. This is the read surface the Python side reaches.
* **gecko's own render machinery** (`gecko_ia2.cpp` +
  `nvda_ia2`) — already Rust-driven: `render`, `fillVBuf`, the WinEvent
  dispatch, `isRootDocAlive`. It mutates the buffer on the render
  thread.

Checked and **cleared** (they do *not* touch vbuf storage/backend):
`ia2LiveRegions.cpp`, `IA2Support.cpp`, `textFromIAccessible.cpp`,
`inProcess.cpp`, `ia2utils.cpp` (the `gecko`/`IA2` matches there are
incidental). `winword.cpp`'s `getTextInRange` is its own Word RPC,
unrelated to vbuf.

So the seam has exactly two sides: **vbufRemote reads** and **gecko's
render-thread writes**. Both serialize on the same `backend->lock`.

## Decision 1 — Node identity: two coherent halves, never a narrowed pointer

Rust nodes are `u64` slotmap keys with no C++ object; the C++ storage
virtuals return/accept `VBufStorage_fieldNode_t*`. The migration keeps
these two identity spaces from ever colliding by splitting the seam:

### 1a. Render/backend-thread side: identity stays inside Rust

Gecko's render, invalidation, reuse, and WinEvent dispatch already run
in Rust (`fill_vbuf.rs`, `gecko_backend_state.rs`). Node identity on
this side is a Rust `NodeKey` throughout — it never becomes a C++
pointer, so there is no width problem. The C++ `GeckoVBufBackend_t`
holds no live storage of its own; the live tree lives in a Rust
`Buffer` embedded in `GeckoBackendState` (Decision 3).

### 1b. vbufRemote read side: route gecko through the u64 `nvda_vbuf_*` ABI

Because the RPC handle is already `u64`, `vbufRemote.cpp` carries the
Rust key verbatim for gecko and a narrowed C++ pointer for the legacy
backends. Each RPC branches on a new backend accessor:

```cpp
// added to VBufBackend_t (backend.h), default null:
virtual void* getRustStorageBuffer() { return nullptr; }
// GeckoVBufBackend_t override returns the address of the embedded
// Rust Buffer (via nvda_ia2_gecko_backend_get_buffer(rustState)).
```

Each read RPC then reads:

```cpp
int VBufRemote_getControlFieldNodeWithIdentifier(..., VBufRemote_nodeHandle_t* foundNode) {
    VBufBackend_t* backend = (VBufBackend_t*)buffer;
    backend->lock.acquire();
    if (void* rb = backend->getRustStorageBuffer())
        *foundNode = nvda_vbuf_buffer_get_control_field_node_with_identifier(rb, docHandle, ID);
    else
        *foundNode = (VBufRemote_nodeHandle_t)(backend->getControlFieldNodeWithIdentifier(docHandle, ID));
    backend->lock.release();
    return (*foundNode) != 0;
}
```

The `u64` sentinel for "none" is `0` on both sides
(`NVDA_VBUF_NODE_NONE`; a narrowed null pointer is also `0`), so the
existing `!= 0` success tests are unchanged. The Python side never
learns which storage produced the handle: it hands the same `u64` back
on the next RPC, and the branch routes it to the matching storage.

**Why not override the storage virtuals instead (rejected).** Making
`GeckoVBufBackend_t` override `getControlFieldNodeWithIdentifier`,
`locateTextFieldNodeAtOffset`, `findNodeByAttributes`, … forces those
overrides to *return* `VBufStorage_fieldNode_t*` — a 32-bit pointer on
x86 that cannot hold a 64-bit key. The only way to make that work is a
per-buffer `map<u64, heap-stub-node>` so every key has a pointer-shaped
proxy. That table would need invalidation on every `replace_subtrees`
(which deletes and recreates nodes wholesale), leak-free teardown on
`clearBuffer`, and careful lifetime rules for handles Python still
holds across a re-render. It is pure overhead that exists only to
satisfy a C++ signature the RPC layer doesn't actually need — the RPC
handle is `u64`. Rejected.

**Why not special-case with a raw `if (rustBuffer)` and no virtual
(considered, folded in).** The `getRustStorageBuffer()` accessor *is*
that branch key; the virtual is just how the (single) gecko subclass
advertises its Rust buffer without `dynamic_cast`. Keeping the branch
in `vbufRemote.cpp` (rather than spraying `nvda_vbuf_*` calls behind
storage-virtual overrides) keeps all gecko-awareness in one file and
makes the legacy path a literal else-branch — maximally reversible.

### Coverage: every read RPC already has a `u64` extern

Per `2026-07-10-vbufremote-coverage-audit.md`, all twelve read RPCs map
1:1 onto existing `extern_api.rs` functions — **no new `nvda_vbuf_*`
read extern is required**:

| VBufRemote RPC | `nvda_vbuf_*` extern used by the gecko branch |
| --- | --- |
| `getFieldNodeOffsets` | `nvda_vbuf_buffer_field_node_offsets` |
| `isFieldNodeAtOffset` | `nvda_vbuf_buffer_is_field_node_at_offset` |
| `locateTextFieldNodeAtOffset` | `nvda_vbuf_buffer_locate_text_field_node_at_offset` |
| `locateControlFieldNodeAtOffset` | `nvda_vbuf_buffer_locate_control_field_node_at_offset` |
| `getControlFieldNodeWithIdentifier` | `nvda_vbuf_buffer_get_control_field_node_with_identifier` |
| `getIdentifierFromControlFieldNode` | `nvda_vbuf_node_identifier` |
| `findNodeByAttributes` | `nvda_vbuf_buffer_find_node_by_attributes` |
| `getSelectionOffsets` | `nvda_vbuf_buffer_get_selection_offsets` |
| `setSelectionOffsets` | `nvda_vbuf_buffer_set_selection_offsets` |
| `getTextLength` | `nvda_vbuf_buffer_text_length` |
| `getTextInRange` | `nvda_vbuf_buffer_get_text_in_range` (callback → `SysAllocString`) |
| `getLineOffsets` | `nvda_vbuf_buffer_line_offsets` |

`getTextInRange` is the only one needing a shim detail: the extern
delivers its string through an `NvdaVbufStringCallback`; the RPC passes
a callback that does `*text = SysAllocString(...)` and returns
`true`, preserving the empty-string-returns-false contract.

`createBuffer` / `destroyBuffer` / `bufferHandle_t_rundown` are backend
lifecycle, untouched (they already construct a `GeckoVBufBackend_t` via
the factory; the embedded Rust `Buffer` is created/destroyed with the
`GeckoBackendState`, Decision 3).

## Decision 2 — Locking and the `Send`/`Sync` argument

**The lock does not move.** `backend->lock` (a `LockableObject` on the
C++ `VBufBackend_t`) remains the one synchronization primitive. Every
vbufRemote RPC acquires it before touching the Rust buffer (the branch
above sits *inside* the existing `lock.acquire()/release()`), exactly
as it wraps the C++ virtual today. The render thread acquires the same
lock around its mutations (Decision 3's `update()` override calls
`this->lock.acquire()/release()`, matching the C++ `update()` it
replaces).

**Why the Rust `Buffer` stays sound across threads.** `Buffer`
(`storage/mod.rs:106`) is composed only of owned, `Send` data —
`SlotMap<NodeKey, Node>`, `BTreeMap`, `Vec`, `i32`. It contains no raw
pointers, `Rc`, `Cell`, or `RefCell`, so it is automatically `Send` and
is *not* required to be `Sync`. The extern functions receive a
`*mut Buffer` / `*const Buffer` and materialize a `&mut Buffer` / `&Buffer`
**only for the duration of a single call**, never stored. Because
`backend->lock` guarantees mutual exclusion between the RPC threads and
the render thread, no two threads ever hold a reference to the same
`Buffer` simultaneously, and no Rust reference outlives its locked
region. This is the classic "`Send`, guarded by an external mutex"
pattern: ownership is effectively transferred into whichever thread
holds the lock, and the `&mut` aliasing invariant holds because the
lock serializes access. The one intra-render aliasing hazard (main vs
temp buffer during reuse) is a *different* pair of allocations and is
guarded by the `debug_assert!` distinct-pointer check already in
`nvda_vbuf_buffer_reuse_existing_node` / the reuse path (Decision 4).

## Decision 3 — Update/render orchestration: `update()` becomes virtual, gecko re-homes it onto Rust

### The Rust `Buffer` lives on `GeckoBackendState`

```rust
pub struct GeckoBackendState {
    pub toolkit_name: Vec<u16>,
    pub root_doc_acc: Option<IAccessible2>,
    pub buffer: Buffer,   // NEW (Phase 6e): the live gecko storage
}
```

Embedding (rather than a separate `nvda_vbuf_buffer_create` allocation
held by the C++ class) resolves the 6d open question: `GeckoBackendState`
already owns the per-backend Rust state and is created/destroyed in
`GeckoVBufBackend_t`'s ctor/dtor, so the buffer's lifetime is automatic
and there is one fewer raw pointer to track. The C++ side reaches it
through `nvda_ia2_gecko_backend_get_buffer(state) -> *mut Buffer`.

### `VBufBackend_t::update()` becomes virtual

`update()` is currently a **non-virtual** protected method on
`VBufBackend_t` (`backend.h:111`), called from three base-class sites:
`renderThread_timerProc`, `renderThread_initialize`, and `forceUpdate`.
It drains `pendingInvalidSubtreesList` into C++ temp buffers via
`render()` and merges with `replaceSubtrees` on the C++ storage.

We make it `virtual`. The four other backends inherit the base impl
unchanged (zero behavior change — they don't override it). Gecko
overrides it to run the **Rust** orchestration against `state.buffer`.
Because `forceUpdate()`, the timer proc, and `renderThread_initialize`
all reach `update()` through the (now virtual) dispatch, gecko's
override is picked up at every entry point automatically — including
the initial render (`renderThread_initialize` → `update()`).

### The gecko `update()` override

```cpp
void GeckoVBufBackend_t::update() {          // override
    this->lock.acquire();
    nvda_ia2_gecko_backend_update(this->rustState, this);
    this->lock.release();
    nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
}
```

`nvda_ia2_gecko_backend_update` reimplements the `backend.rs::update`
algorithm directly over the embedded `Buffer`, in raw-pointer form so
the FFI reuse lookups work (see Decision 4):

* If `state.buffer` has no content: initial render — `fill_vbuf` writes
  straight into `state.buffer`.
* Else: `take_pending_into_working()`; for each invalid key, look up its
  identifier, create a temp `Buffer`, `fill_vbuf` into the temp (reuse
  queries `state.buffer`), collect `(key, temp)`; then `clear_working()`
  and `state.buffer.replace_subtrees(map)`.

The `vbufChangeNotify` call stays C++-side (it belongs to the Win32
integration layer, exactly as `backend.rs::update`'s doc-comment
already notes it was deliberately left out of the storage-side loop).

**Enumerated virtuals / methods on `GeckoVBufBackend_t` after 6e:**

| Member | Kind | 6e behavior |
| --- | --- | --- |
| `update()` | override (newly virtual) | Rust orchestration over `state.buffer` |
| `getRustStorageBuffer()` | override (new virtual) | `nvda_ia2_gecko_backend_get_buffer(rustState)` |
| `render(buffer, docHandle, ID, oldNode)` | override (pure-virtual, kept concrete) | vestigial — no longer on the live path once `update()` is overridden; kept as a valid stub so the class stays instantiable. Its former body moved into the Rust renderer. |
| `renderThread_initialize` | override (unchanged) | base call drives the initial `update()`; then stashes `root_doc_acc` |
| `renderThread_terminate` | override (unchanged shape) | base call `clearBuffer`s the (empty) C++ storage; the Rust `terminate` extern now also clears `state.buffer` |
| `versionSpecificInit`, WinEvent hook, ctor/dtor | unchanged | as today |

`invalidateSubtree` and `forceUpdate` are **not** overridden. Gecko
never invalidates through the C++ virtual: its only invalidation caller
is the Rust `dispatch_win_event`, which operates on `state.buffer`
directly (Decision 5). `forceUpdate` stays the base impl —
`cancelPendingUpdate(); update();` — and reaches the Rust orchestration
through the now-virtual `update()`.

`clearBuffer` is **not** made virtual. Its two live sites for gecko are
covered without it: the Rust `terminate` extern clears `state.buffer`,
and the WinEvent defunct-document path clears it directly (Decision 5).
The base `renderThread_terminate` still calls the inherited
`clearBuffer()` on gecko's (always-empty) C++ storage, which is a
harmless no-op.

### New externs for orchestration (C++ ⇄ Rust)

All defined in `nvda_ia2` except the two thin C-shim helpers:

| Extern | Direction | Purpose |
| --- | --- | --- |
| `nvda_ia2_gecko_backend_get_buffer(state) -> *mut Buffer` | C++→Rust | address of `state.buffer`; used by `getRustStorageBuffer` and vbufRemote |
| `nvda_ia2_gecko_backend_update(state, backend)` | C++→Rust | drain/render/merge orchestration over `state.buffer` |
| `nvda_ia2_gecko_backend_clear_buffer(state)` | C++→Rust | `state.buffer.clear()` (called from the terminate path) |
| `vbuf_backend_request_update(backend)` | Rust→C++ (c_shim) | calls `backend->requestUpdate()` (arms the render-thread timer) after a Rust-side invalidation |
| `vbuf_backend_get_rust_storage_buffer(backend) -> *mut Buffer` | Rust→C++→Rust (c_shim) | `backend->getRustStorageBuffer()`; lets `nvda_ia2` reach the buffer from a bare `VBufBackend_t*` where it lacks `state` |

`requestUpdate()` is promoted from `protected` to `public` on
`VBufBackend_t` (or given a public forwarder) so the c_shim free
function can call it. No new `nvda_vbuf_*` C ABI is added:
`replace_subtrees`, `reuse_existing_node`, and the `update` loop stay
**inside Rust** (`nvda_ia2` depends on `nvda_vbuf` as a cargo crate and
calls `Buffer` methods directly — no extern hop), which is why 6d-a's
deferral of a C ABI for those is permanent, not a debt.

## Decision 4 — `fillVBuf` renders into the Rust `Buffer`; the reuse dual-borrow

6e turns the `direct_rust_storage` feature **on** for `nvda_ia2`'s
dependency on `nvda_vbuf`. Under the feature the newtypes (`VbufBuffer`,
`VbufFieldNode`, `VbufControlFieldNode`) already wrap the Rust `Buffer`
/ a `(buffer, key)` `NodeRef` (6d-b). So the entire `fill_vbuf.rs` tree
— thousands of `add_control_field_node` / `add_attribute` calls —
retargets onto the Rust `Buffer` **with no source change**, purely by
flipping the feature. This is the payoff of the 6d-b design.

### What 6d-b compiled out, and what replaces it

6d-b removed three `VbufBackend` methods under the feature
(`as_buffer`, `invalidate_subtree`, `reuse_existing_node`) because a C++
backend pointer is not a Rust `Buffer`. 6e does **not** restore them on
`VbufBackend`. Instead their two callers are re-homed:

1. **`fill_vbuf` block1's cross-buffer reuse.** `FillVBufCtx` gains a
   `main: VbufBuffer` field (the backend's live buffer, set once by the
   orchestration from `state.buffer`). Block1's

   ```rust
   if buffer.0 != backend.as_buffer().0 { … backend.reuse_existing_node(…) … }
   ```

   becomes

   ```rust
   if buffer.0 != ctx.main.0 {
       if let Some(existing) = ctx.main.reuse_existing_node_in_render(buffer /*temp*/, Some(parent_node), previous, doc_handle, id) { … }
   }
   ```

   where `VbufBuffer::reuse_existing_node_in_render(self /*main*/, temp,
   parent, previous, doc, id)` is a new feature-on newtype method that
   calls `(*main).reuse_existing_node_in_render(&*temp, parent.key,
   previous.key, doc, id)`. This is the FFI shape for the
   dual-buffer borrow the 6d plan flagged as open:

   * **main** is `ctx.main.0` — a `*mut Buffer` pointing at
     `state.buffer`.
   * **temp** is `buffer.0` — the in-flight render's `*mut Buffer`.
   * They are always distinct allocations (initial render skips reuse
     because `buffer.0 == ctx.main.0`). The reuse method takes
     `&mut main` and `&temp` for the call only; the orchestration never
     holds a competing `&mut main` across the `fill_vbuf` call (it works
     in raw pointers and only re-borrows `state.buffer` at
     `replace_subtrees` time), so there is no aliasing of the main
     buffer. A `debug_assert!` on distinct pointers documents the
     contract.

   The C++ `reuseExistingNodeInRender`'s side effect — erasing the
   reused node from `workingInvalidSubtreesList` — is preserved because
   `Buffer::reuse_existing_node_in_render` mutates `self`'s
   `working_invalid` (verified in `storage/mod.rs`; it fully mirrors the
   C++ contract including the `denyReuseIfPreviousSiblingsChanged` and
   `allowReuseInAncestorUpdate` logic).

2. **`dispatch_win_event` / `is_root_doc_alive` backend ops** — moved
   onto `state.buffer` (Decision 5).

## Decision 5 — WinEvent dispatch on `state.buffer`

`nvda_ia2_gecko_backend_dispatch_win_event` and
`nvda_ia2_gecko_backend_is_root_doc_alive` currently reach storage
through `VbufBackend`'s (feature-off) methods. Under 6e they use
`state.buffer` directly (they already have `state`), plus two small C++
externs for the timer/root:

| Old call | 6e replacement |
| --- | --- |
| `backend_h.as_buffer().get_control_field_node_with_identifier(dh,id)` | `state.buffer.get_control_field_node_with_identifier(dh,id)` |
| `node.set_always_rerender_descendants(true)` | `state.buffer.get_mut(key)` → set flag |
| `backend_h.invalidate_subtree(node)` | `state.buffer.invalidate_subtree(key)`, then `vbuf_backend_request_update(backend)` if it returned `true` |
| `backend_h.clear_buffer()` | `state.buffer.clear()` |
| `backend_h.force_update()` | `vbuf_backend_force_update(backend)` (C++ `forceUpdate` → virtual `update()` → Rust) |
| `backend_h.pending_invalid_subtrees_empty()` | `state.buffer.pending_invalid_subtrees_empty()` |
| `backend_h.root_doc_handle()/root_id()` | `vbuf_backend_get_root_doc_handle/id(backend)` (unchanged externs) |

This keeps invalidation identity in Rust keys end-to-end, and the timer
(the one genuinely-Win32 piece) reached through `requestUpdate`. The
`invalidate_subtree` → `request_update` split faithfully reproduces the
C++ `invalidateSubtree` … `requestUpdate()` tail.

## Decision 6 — Keepalive retires at the flip

`rust/nvda_ia2/src/vbuf_keepalive.rs` exists solely to stop the linker
from eliding the `nvda_vbuf_*` externs while they have no real callers.
After the flip, `vbufRemote.cpp` (the read RPCs) and the orchestration
reference them from C++ with real callers, so dead-code elimination no
longer strips them. The module — and its `count == 38` test — is
**deleted at the flip commit**. (If a belt-and-braces margin is wanted,
delete it one commit later, after confirming the symbols survive the
DLL link; but the flip is the natural point.)

## Staged plan

Each stage compiles, passes `cargo test -p nvda_vbuf` (both feature
configs) + `cargo test -p nvda_ia2`, and links the DLL. Only **Stage D**
changes runtime behavior.

> Build note (per repo memory): a new `.rs` FFI export needs a
> `cargo build --target-dir build/rust` pass before `scons` so scons
> does not link a stale `nvda_ia2.lib`. Fresh worktrees can't do the
> full DLL link (submodules + venv + liblouis); the link-gate stages
> run in the main clone.

### Stage A — C++ scaffolding, dormant (no behavior change)

* `nvdaHelper/vbufBase/backend.h`: make `void update();` → `virtual
  void update();`; promote `requestUpdate()` to `public` (or add a
  public forwarder); add `virtual void* getRustStorageBuffer() { return
  nullptr; }`.
* `nvdaHelper/vbufBase/c_shim.cpp`: add `vbuf_backend_request_update`
  and `vbuf_backend_get_rust_storage_buffer`.
* Everything still routes to C++ storage; `getRustStorageBuffer`
  returns null everywhere; nothing calls the new externs. Bit-for-bit
  identical runtime behavior. **Stopping point:** virtual dispatch and
  the accessor exist, unused.

### Stage B — Rust storage ownership + orchestration, feature-gated dormant

* `rust/nvda_ia2/src/gecko_backend_state.rs`: add `buffer: Buffer` to
  `GeckoBackendState` (present in both configs; empty until the flip).
* Add `nvda_ia2_gecko_backend_get_buffer`,
  `nvda_ia2_gecko_backend_update`, `nvda_ia2_gecko_backend_clear_buffer`
  externs and the Rust drain/render/merge orchestration. Gate the
  orchestration body and the feature-on `FillVBufCtx.main` plumbing on
  `#[cfg(feature = "direct_rust_storage")]` so it type-checks only when
  the newtypes are Rust-backed; provide trivial `#[cfg(not(...))]`
  fallbacks (or leave the externs `cfg`-gated with C++-side stubs) so
  the feature-off `nvda_ia2` build is unaffected.
* `rust/nvda_vbuf/src/lib.rs`: add the feature-on
  `VbufBuffer::reuse_existing_node_in_range`/`reuse_existing_node_in_render`
  newtype method (Decision 4). Add `vbuf_backend_request_update` /
  `vbuf_backend_get_rust_storage_buffer` extern declarations.
* `nvda_ia2` remains feature-**off**, so its DLL contribution is
  unchanged; the new Rust code is exercised by
  `cargo test -p nvda_vbuf --features direct_rust_storage` and
  `cargo test -p nvda_ia2` (feature-off compile of the fallbacks).
  **Stopping point:** all machinery exists and is unit-tested; live
  gecko still on C++ storage.

### Stage C — vbufRemote gecko branch, dormant

* `nvdaHelper/remote/vbufRemote.cpp`: add the `if
  (backend->getRustStorageBuffer())` branch to each of the twelve read
  RPCs, plus the `getTextInRange` callback shim.
* Still dormant: `getRustStorageBuffer` returns null (gecko hasn't
  overridden it yet), so every RPC takes the legacy C++ else-branch.
  Runtime behavior unchanged. The DLL now *references* the
  `nvda_vbuf_buffer_*` read externs, so they are no longer elision
  candidates. **Stopping point:** read routing in place, still inert.

### Stage D — THE FLIP (single behavior-changing commit; smoke-test gate)

* `rust/nvda_ia2/Cargo.toml`: enable `direct_rust_storage` on the
  `nvda_vbuf` dependency. This activates the Rust-backed newtypes,
  the `cfg`-gated orchestration, and `FillVBufCtx.main`.
* `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.{h,cpp}`:
  * override `update()` → `nvda_ia2_gecko_backend_update`;
  * override `getRustStorageBuffer()` →
    `nvda_ia2_gecko_backend_get_buffer(rustState)`;
  * clear `state.buffer` on `renderThread_terminate`;
  * repoint `render`/`fillVBuf` wiring so the orchestration passes the
    Rust buffer pointer (`state.buffer` / temp) rather than the C++
    `void* buffer`.
* Delete `rust/nvda_ia2/src/vbuf_keepalive.rs` and its `mod` line.
* **This is the only commit where a live gecko document renders into,
  and reads out of, the Rust `Buffer`.** Gate on a Firefox + Chrome
  smoke test (arrow-key reading, headings/links navigation, a table, a
  live region / dynamic update, focus tracking, find-in-page).

### Stage E — cleanup (optional, post-settle)

Once 6e is comfortable, `render()`'s vestigial gecko override and the
now-dead feature-off branches inside `nvda_ia2` can be pruned. Phase 6f
(delete `gecko_ia2.cpp`, move the factory into Rust) proceeds from here,
as previously planned.

## Reversal path

The C-shim (`c_shim.cpp`) and the C++ `VBufStorage_buffer_t` /
`VBufBackend_t` storage remain in the tree throughout — exercised by the
four other backends at all times, and by gecko itself until Stage D. The
flip is **one commit**; reversing gecko to the C++ storage is
`git revert` of Stage D:

* re-disabling `direct_rust_storage` reverts `fill_vbuf` to render
  through the C-shim into C++ storage;
* removing gecko's `update()` / `getRustStorageBuffer()` overrides sends
  `update()` back to the base C++ orchestration and every vbufRemote RPC
  back to its legacy else-branch (since `getRustStorageBuffer()` is null
  again);
* the keepalive can be restored with the same revert, or left out (the
  read externs now have C++ callers via Stage C, which is *not* reverted).

Stages A–C are behavior-preserving and need not be reverted to restore
the old runtime behavior — only Stage D flips live behavior, so only
Stage D must be reverted. That is the concrete meaning of the 6d plan's
"route gecko_ia2 back through the C-shim by reverting the wiring."

## What stays C++ indefinitely (unchanged from the 6d plan)

* `VBufBackend_t`'s Win32 render-thread machinery — timer
  (`SetTimer`/`requestUpdate`), `WH_CALLWNDPROC` + WinEvent destroy
  hooks, `execInThread`, `runningBackends`, `rootDocHandle`/`rootID`,
  and the `LockableObject`. Gecko's Rust code drives *what* to render
  and *where to store it*; C++ still owns *when* (scheduling) and the
  thread affinity.
* The four non-gecko backends and their C++ storage.

## Open questions

None blocking. Two implementation-time confirmations (not design
choices):

* **`render()` override disposition.** Keeping a vestigial concrete
  `render()` (so the class stays instantiable) vs. restructuring the
  Rust orchestration to call *back* through it. The doc chooses the
  vestigial stub; if implementation finds a caller of `render()` we
  missed, repoint it — but the consumer inventory found none.
* **Exact `cfg` seams in Stage B** (which fallbacks are `#[cfg(not)]`
  stubs vs. `cfg`-gated externs) is a mechanical choice to keep the
  feature-off `nvda_ia2` build green; it does not affect the shipped
  (feature-on) behavior.
