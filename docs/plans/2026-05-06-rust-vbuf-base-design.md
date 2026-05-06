# Design: port `vbufBase` to Rust (Phase 6)

**Status:** design (2026-05-06). No code yet.

This is the Phase 6 follow-up to `2026-05-05-rust-gecko-ia2-roadmap.md`.
It scopes the port of `nvdaHelper/vbufBase/` (~2700 lines of C++)
into Rust.

## Motivation

After Phases 1-5, gecko_ia2 backend logic lives in Rust but reaches
the storage layer through a C-shim (`nvdaHelper/vbufBase/c_shim.cpp`)
that wraps the C++ `VBufStorage_*` and `VBufBackend_t` classes.
Phase 6 ports those classes themselves to Rust, eliminating the
shim and giving Rust callers direct access to the storage layer.

End-state goals:

* `nvdaHelper/vbufBase/c_shim.cpp` deleted.
* `VBufStorage_*` and `VBufBackend_t` are Rust types.
* `gecko_ia2` calls Rust storage directly without crossing FFI per
  vbuf operation.
* The remaining C++ backends (mshtml / webKit /
  lotusNotesRichText / adobeAcrobat) continue working through a
  thin C++ adapter that re-exposes the same shape they consume
  today.

## What the C++ side looks like

`nvdaHelper/vbufBase/`:

* `storage.h/cpp` (~1900 lines) — node tree, buffer.
* `backend.h/cpp` (~530 lines) — polymorphic backend.
* `utils.h/cpp` (~250 lines) — utility helpers.

Class hierarchy:

```text
VBufStorage_fieldNode_t (abstract, has parent/prev/next/firstChild
                         /lastChild + attributes + virtual methods)
├── VBufStorage_controlFieldNode_t (docHandle/ID identifier,
│                                  rerender/reuse flags)
│   └── VBufStorage_referenceNode_t (alias node pointing at another
│                                   control node)
└── VBufStorage_textFieldNode_t (leaf with text)

VBufStorage_buffer_t (owns nodes, manages tree, navigation,
                      selection, serialization, locking)

VBufBackend_t : public VBufStorage_buffer_t (polymorphic; render
                                            thread; pending /
                                            working invalid lists)
```

Active C++ inheritors of `VBufBackend_t`:

* `GeckoVBufBackend_t` (gecko_ia2) — already a thin Rust adapter
  after Phase 5.
* `MshtmlVBufBackend_t` (mshtml).
* `WebKitVBufBackend_t` (webKit).
* `LotusNotesRichTextVBufBackend_t` (lotusNotesRichText).
* `AdobeAcrobatVBufBackend_t` (adobeAcrobat).

## Key design questions

### 1. Tree representation

The C++ tree uses raw pointers everywhere (`parent`, `previous`,
`next`, `firstChild`, `lastChild`). Three Rust options:

**(a) Reference-counted tree** (`Rc<RefCell<Node>>` for parents,
`Weak` for back-edges). Easy to port the existing pointer logic,
but `RefCell` borrow panics replace use-after-free crashes — same
class of bug, different debug experience.

**(b) Arena-allocated tree** (slab/index-based; nodes hold integer
indices into a `Vec<Node>` rather than pointers). Zero unsafe in
the tree itself; cache-friendly. The buffer owns the arena.
Children/siblings are `Option<NodeIndex>`. This is the idiomatic
Rust pattern for graph-like structures.

**(c) Raw pointers + unsafe Rust** (mirror C++ exactly). Loses
most of the safety win.

**Recommendation:** Arena (option b). The pointer ownership model
in C++ is "buffer owns all nodes; nodes refer to each other via
raw pointers but only the buffer can free them." That maps to "Vec
owns Node values; nodes refer to each other by index" exactly.

### 2. Polymorphism on field nodes

The C++ uses virtual methods on `VBufStorage_fieldNode_t`:
`generateMarkupTagName`, `locateTextFieldNodeAtOffset`,
`getTextInRange`, `disassociateFromBuffer`. Subclasses override.

**Recommendation:** Use a Rust enum:

```rust
enum FieldNodeKind {
    Control(ControlFieldData),
    Text(TextFieldData),
    Reference(ReferenceData),
}
```

Closed set, dispatches via `match`. Cleaner than `Box<dyn>` for a
fixed type set.

### 3. Polymorphism on backends (`VBufBackend_t`)

Backends are extension points: each backend type (gecko_ia2,
mshtml, ...) implements `render()` differently. The C++ uses
abstract `virtual void render(...) = 0`.

**Recommendation:** Use a Rust trait `VBufBackend`. Implementors
include:

* `GeckoBackend` (Rust, replacing the current C++ class).
* For each remaining C++ backend, a thin C++ adapter that
  inherits from a still-existing-but-thin C++ `VBufBackend_t`
  shell. The Rust trait dispatches into that adapter via a
  function pointer set up at `*_createInstance` time.

This keeps existing C++ backends working without porting them.
They become "C++ render callbacks plugged into the Rust backend
infrastructure."

### 4. C++ backend inheritance

This is the trickiest piece. mshtml et al. currently inherit from
`VBufBackend_t` (a C++ class) and call its protected members
(`pendingInvalidSubtreesList`, etc.) and override its virtuals.

If `VBufBackend_t` becomes a Rust struct, those C++ inheritances
break.

**Solutions ordered by ambition:**

* **Solution A (least disruptive):** Keep a stub `VBufBackend_t`
  C++ class with empty/no-op bodies. Real state lives in Rust. The
  C++ subclasses inherit from the stub, override `render()`, and
  the Rust storage layer dispatches into the stub's render via a
  C ABI function pointer.
* **Solution B (medium):** Generate a C wrapper `VBufBackend_c`
  that's a struct of function pointers. Each existing C++ backend
  is rewritten as a few C functions populating that struct.
* **Solution C (most ambitious):** Port all backends to Rust. Most
  work; eliminates all C++ backend code.

Solution A is the right starting point.

### 5. Threading and locking

`VBufBackend_t::lock` is a `LockableObject`. The render thread
also uses a `SetWinEventHook` callback. Rust's
`std::sync::Mutex` and the existing render-thread plumbing
should slot in fine, but care is needed: the lock is exposed
publicly and held across calls from outside vbufBase. That
contract has to hold across the FFI boundary.

### 6. The C-shim API surface

The existing C-shim (`nvdaHelper/vbufBase/c_shim.cpp`) exposes
~25 functions. Two paths:

* **Eliminate it for Rust callers.** gecko_ia2 (Rust) calls Rust
  storage directly. The C-shim file is deleted.
* **Keep it for C++ callers.** mshtml et al. continue using the
  C-shim, which now targets Rust storage internally rather than
  C++ classes.

We'll keep the C-shim but flip its implementation: it will now be
written in Rust (`pub unsafe extern "C"` over the new Rust
storage), not C++. The C-shim stays alive as long as any C++
backend exists.

## Phased plan

The total work is multi-week. Each sub-phase is a coherent commit
chain that leaves the tree green:

### Phase 6a — pure-logic helpers (~1 day)

Port `utils.cpp`'s pure helpers to Rust:

* `getNameForURL` (already done — `name_for_url.rs`).
* `multiValueAttribsStringToMap` (used by mshtml; can keep C++ for
  now since mshtml stays C++).
* `nodeHasUsefulContent` / `nodeContentMatchesString` (already
  Rust-callable via the C-shim; will be re-implemented Rust-side
  once nodes are Rust).
* `isWhitespace` / `isPrivateCharacter`.

Most of this is already in place.

### Phase 6b — Rust storage types (~1 week)

Implement `FieldNodeKind` enum + `ControlFieldData` / `TextFieldData`
/ `ReferenceData` structs. Implement `Buffer` with arena-based
node ownership.

Port:

* `addControlFieldNode`, `addTextFieldNode`,
  `addReferenceNodeToBuffer`.
* `getControlFieldNodeWithIdentifier`, `getControlFieldNodes`.
* `isDescendantNode`, `isNodeInBuffer`.
* `getTextInRange`, `getLineOffsets`, `getSelectionOffsets`,
  navigation helpers.
* Markup generation (`generateMarkupOpeningTag`,
  `generateMarkupClosingTag`).
* Attribute storage / lookup.

Storage's tests (if any) come along.

### Phase 6c — Rust `VBufBackend` trait + adapter (~3 days)

Define the `VBufBackend` trait. Implement the render-thread
machinery in Rust (`requestUpdate`, `forceUpdate`,
`invalidateSubtree`, `reuseExistingNodeInRender`, the timer
proc, the destroy WinEvent hook).

Build the C++ adapter for non-ported backends: a thin
`VBufBackend_t` C++ class whose body is a one-liner per method
that calls into Rust. Existing C++ backends keep inheriting from
it.

### Phase 6d — flip the C-shim to Rust (~2 days)

Reimplement `nvdaHelper/vbufBase/c_shim.cpp` as a Rust file
(`rust/nvda_vbuf/src/c_shim.rs`) that exposes the same `extern
"C"` functions but routes them through the new Rust storage. The
C++ file is deleted.

### Phase 6e — migrate gecko_ia2 to direct Rust calls (~2 days)

`nvda_ia2` already has Rust bindings around the C-shim. This step
swaps those bindings for direct calls into the new Rust storage
types. C-shim stays for the still-C++ backends.

### Phase 6f — collapse the C++ adapter for gecko_ia2 (~1 day)

Now that nothing else needs `GeckoVBufBackend_t`'s C++ shape, the
factory function `GeckoVBufBackend_t_createInstance` can move
Rust-side and `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp`
can be deleted. The adapter in Phase 6c handles polymorphism.

### Phase 6g (optional, much later) — port remaining C++ backends

mshtml / webKit / lotusNotesRichText / adobeAcrobat. Each is its
own multi-day effort. Stops being optional only when we want the
NVDA codebase to be "no C++ vbuf backends at all".

## Stopping points

The plan has clear stopping points:

* After **6b**: Rust storage works in isolation. Bonus tests for
  the data structure. C-shim still calls C++ classes; nothing
  visible changes for callers.
* After **6c**: Rust backend infrastructure works. Nothing wired
  in.
* After **6d**: C-shim is Rust-implemented but observably
  identical. **First user-visible change** — bugs surface here.
* After **6e**: gecko_ia2 uses Rust storage natively. Performance
  may improve (less FFI per vbuf operation).
* After **6f**: gecko_ia2.cpp is gone.
* After **6g**: vbufBase C++ classes are gone (except the thin
  adapter for any remaining unported backends).

The "right" stopping point depends on appetite. **Through 6f** is
already a big win and a good place to declare victory.

## Open questions

* **Arena type:** roll our own `Vec<Node>` + free-list, or use
  `slab` / `slotmap` from crates.io? `slotmap` has generational
  indices that catch dangling references at runtime — useful for
  porting C++ raw-pointer logic safely. Probably worth the dep.
* **Locking shape:** the C++ `LockableObject` is a recursive
  CRITICAL_SECTION. Rust's `parking_lot::ReentrantMutex` matches.
  Can we keep the existing semantics with `std::sync::Mutex`
  (non-reentrant)? Audit needed.
* **`VBufStorage_relativeSelection_t`** is exposed as a typedef
  outside vbufBase. Is anyone outside vbufBase using it? Quick
  grep across the NVDA codebase before locking the API.
* **Markup generation format:** the current `generateMarkup*`
  output is consumed by the Python side. Need byte-identical
  output to avoid behavioral drift; capture a few sample outputs
  before porting and use them as snapshot tests.
* **Multi-thread safety of the WinEvent hook removal:** the C++
  destroys backends from a `WH_CALLWNDPROC` hook. Rust drops are
  automatic but the timing might differ; behavior on
  `EVENT_OBJECT_DESTROY` for the root window needs verification.

## Out of scope

* Porting the C++ vbuf backends themselves (Phase 6g, deferred).
* Refactoring the Python ↔ NVDAHelper API on the Python side.
* Performance work beyond what falls out naturally from the port.
