# Roadmap: fully-Rust `gecko_ia2` vbuf backend

**Status:** Phase 1 in progress (2026-05-05)

## End-state goal

`nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp` deleted. All gecko-IA2 vbuf rendering happens in Rust. A ~30-line C++ adapter survives at Phase 5 as a `VBufBackend_t` polymorphic-class shim; eliminated at Phase 6 if we also port `vbufBase` to Rust.

## Current state (2026-05-05, after the recent porting batch)

* `gecko_ia2.cpp` is 1540 lines; ~360 ported (~23%).
* Helpers fully ported to Rust (Rust impl + C++ thin delegation): `IAccessible2FromIdentifier`, `getSelectedItem`, `getRelationElementsOfType`, `getTextBoxInComboBox`, `getRoleLongRoleString`, `getLabelInfo`, `getChildCount`, `getAccDescription`.
* `fillVBuf` (~854 lines) and the render-thread machinery untouched. Bulk of remaining C++.

## Inventory of remaining C++ in `gecko_ia2.cpp`

| Group | Members | Lines | Blocker |
| --- | --- | --- | --- |
| A | `createMapOfIA2AttributesFromPacc`, `getIAccessible2UniqueID` | ~30 | none -- mechanical eliminations once Phases 1-2 land |
| B | `getTableIDFromCell`, `isRootDocAlive`, `getAllRelationIdsForRelationType` | ~50 | needs `IAccessibleTableCell` binding |
| C | `fillTableHeaders`, `fillTableCellInfo_IATable2`, `fillVBufAriaDetails`, `fillVBufAriaError`, `_extendDetailsRolesAttribute`, `versionSpecificInit` | ~150 | needs vbuf shim (Phases 1+2) |
| D | `renderThread_winEventProcHook`, `renderThread_initialize/_terminate`, `render`, ctor / dtor | ~120 | needs vbuf shim + factory plumbing |
| E | `fillVBuf` | ~854 | needs vbuf shim + design doc of its own |
| F (skip) | `hasXmlRoleAttribContainingValue`, `hasAriaHiddenAttribute` | ~10 | pure logic over a C++-owned attribute map; FFI overhead too high until the map representation moves to Rust |

## Roadmap

### Phase 1 -- vbuf C-shim layer (this PR; ~1 day)

`nvdaHelper/vbufBase/c_shim.cpp` exposes the ~25 vbuf operations gecko_ia2 needs as flat `extern "C"` functions. Mechanical wrapping of `VBufStorage_*` C++ class methods. `vbufBase/storage.cpp` itself is unchanged. The shim file gets compiled into every `nvdaHelperRemote.dll` arch alongside the existing vbuf sources.

Surface (design):

```c
/* --- buffer-level operations ---------------------------------------- */
void* vbuf_buffer_add_control_field_node(
    void* buffer, void* parent, void* previous,
    int doc_handle, int id, int is_block);
void* vbuf_buffer_add_text_field_node(
    void* buffer, void* parent, void* previous,
    const wchar_t* text_ptr, size_t text_len);
void* vbuf_buffer_add_reference_node(
    void* buffer, void* parent, void* previous, void* node);
void* vbuf_buffer_get_control_field_node_with_identifier(
    void* buffer, int doc_handle, int id);
int   vbuf_buffer_is_descendant_node(
    void* buffer, void* parent, void* descendant);
int   vbuf_buffer_is_node_in_buffer(void* buffer, void* node);

/* --- field-node operations ------------------------------------------ */
int   vbuf_node_add_attribute(
    void* node,
    const wchar_t* name_ptr, size_t name_len,
    const wchar_t* value_ptr, size_t value_len);
/* getAttribute -- returns string via callback (BSTR-style FFI we already
   use for getAccDescription). */
int   vbuf_node_get_attribute(
    void* node,
    const wchar_t* name_ptr, size_t name_len,
    void* ctx, vbuf_string_callback cb);
/* getAttributesString -- whole "name:value;..." string via callback. */
void  vbuf_node_get_attributes_string(
    void* node, void* ctx, vbuf_string_callback cb);
int   vbuf_node_get_length(void* node);
int   vbuf_node_is_block(void* node);
int   vbuf_node_is_hidden(void* node);

/* --- field-node bool field setters (gecko writes these directly) ---- */
void  vbuf_node_set_always_rerender_descendants(void* node, int value);
void  vbuf_node_set_always_rerender_children(void* node, int value);
void  vbuf_node_set_deny_reuse_if_previous_siblings_changed(
    void* node, int value);
void  vbuf_node_set_requires_parent_update(void* node, int value);

/* --- VBufBackend_t operations --------------------------------------- */
int   vbuf_backend_get_root_doc_handle(void* backend);
int   vbuf_backend_get_root_id(void* backend);
void  vbuf_backend_clear_buffer(void* backend);
void  vbuf_backend_force_update(void* backend);
void  vbuf_backend_invalidate_subtree(void* backend, void* node);

/* --- string callback -- shared with other gecko_ia2 ports ----------- */
typedef void (*vbuf_string_callback)(
    void* ctx, const wchar_t* ptr, size_t len);
```

`matchAttributes` is intentionally **not** wrapped: its `wregex` parameter is awkward to thread through C, and gecko_ia2 only uses one constant `ATTRLIST_ROLES`/`REGEX_PRESENTATION_ROLE` pair. The Rust port can call `vbuf_node_get_attributes_string` and run the regex (via the `regex` crate) Rust-side.

### Phase 2 -- Rust vbuf bindings (~1 day)

A new module (probably in a new `nvda_vbuf` crate, or under `nvda_ia2` if we keep the workspace flat) wraps the shim functions in opaque-handle newtypes and safe method wrappers:

```rust
pub struct VbufBuffer(*mut c_void);
pub struct VbufFieldNode(*mut c_void);
pub struct VbufControlFieldNode(*mut c_void);
pub struct VbufBackend(*mut c_void);
```

Each handle type is `Send` only if the caller holds the C++ buffer's lock invariants (matches existing C++ practice -- vbuf ops happen on the render thread). Method wrappers translate `Result<>` errors into the `int` boolean returns the shim uses.

### Phase 3 -- easy gecko_ia2 helpers (Groups A + B + C; ~3 days)

With vbuf callable from Rust, the smaller helpers port mechanically. Order:

1. Group A (refactor away `createMapOfIA2AttributesFromPacc`, `getIAccessible2UniqueID`).
2. Group B (`isRootDocAlive`, `getAllRelationIdsForRelationType`, `getTableIDFromCell` -- the latter needs an `IAccessibleTableCell` interface binding alongside).
3. Group C (`fillTableHeaders`, `fillTableCellInfo_IATable2`, `fillVBufAriaDetails`, `fillVBufAriaError`, `_extendDetailsRolesAttribute`, `versionSpecificInit`).

Each commit: Rust port + C++ delegation pair, smoke-tested before pushing (existing workflow).

### Phase 4 -- port `fillVBuf` (~1 week)

The 854-line recursive renderer. Worth its own design doc when we get there. Carve points to consider:

* The IA2-text-segmentation loop (handles `EMBEDDED_OBJ_CHAR`, attribute runs, hyperlink iteration). Heavily uses `IAccessibleText` + the now-Rust `HyperlinkGetter`.
* The IA2-children walk for non-text containers. Uses `getAccessibleChildren` (still C++-only in `ia2utils.cpp`; deferred).
* The role/state-driven attribute building (~200 lines of branching that sets vbuf-node attributes based on IA2 role + state + attribs).
* Table tracking state (parent table / row group, presentationalRowNumber).
* Live region / aria-relevant detection.
* The `renderChildren` skip / `ignoreInteractiveUnlabelledGraphics` flag.

Likely natural carve-up: port the per-IA2-role attribute builder first (pure logic-ish), then the table tracker, then the IA2-text loop, then the recursive children walk. Each lands as its own PR.

Blockers picked up along the way:

* `getAccessibleChildren` (deferred since PR 1) -- needs to be ported or made callable from Rust.
* `IAccessibleAction`, `IAccessibleHypertext`'s `nLinks`, `IAccessibleValue`, `IAccessibleTable2`, `IAccessibleTableCell` -- new interface bindings in the running pile.

### Phase 5 -- render-thread machinery + Rust factory (~2 days)

* `renderThread_winEventProcHook` -- WinEvent handler tied to vbuf-backend state; same shape as the `ia2LiveRegions` port we already did, plus calls to `backend->forceUpdate` / `backend->invalidateSubtree` / `backend->getControlFieldNodeWithIdentifier`.
* `renderThread_initialize` / `_terminate` -- hook lifecycle (Win32 SetWinEventHook / unhook).
* `render` -- public entry, orchestrates `IAccessible2FromIdentifier` (Rust) + `fillVBuf` (Rust by Phase 4).
* Constructor / destructor -- trivial.
* Factory `GeckoVBufBackend_t_createInstance` -- becomes a Rust function exposing a `*mut VBufBackend_t`.

End-state for this phase: `gecko_ia2.cpp` is *gone* (deleted), replaced by a tiny C++ adapter file (probably `gecko_ia2_stub.cpp`, ~30 lines):

```cpp
// Adapter: expose Rust gecko_ia2 logic through the C++ VBufBackend_t
// polymorphic interface that vbufBase/backend.cpp dispatches to.
class GeckoVBufBackend_t : public VBufBackend_t {
    void* rust_state;
public:
    GeckoVBufBackend_t(int docHandle, int ID);
    ~GeckoVBufBackend_t() override;
    void render(...) override;
    // ... forwarding stubs for each VBufBackend_t virtual ...
};

VBufBackend_t* GeckoVBufBackend_t_createInstance(int docHandle, int ID) {
    return new GeckoVBufBackend_t(docHandle, ID);
}
```

This stub exists because `VBufBackend_t` is a C++ polymorphic class -- the vtable dispatch can't live in Rust without either an adapter shim or a port of `VBufBackend_t` itself.

### Phase 6 (optional, separate file) -- port `vbufBase` (~1+ week)

`nvdaHelper/vbufBase/` is 2723 lines of C++:

* `storage.h/cpp` (~1900 lines) -- the field-node tree data structure.
* `backend.h/cpp` (~530 lines) -- the polymorphic backend base + dispatch.
* `utils.h/cpp` (~250 lines) -- helpers.

If we port these to Rust:

* The Phase 1-2 vbuf C-shim is no longer needed (Rust owns the storage types directly).
* The Phase 5 stub disappears -- the factory function lives in Rust, returns an opaque handle, and the dispatcher (mshtml / webKit / lotusNotes / adobeAcrobat / gecko_ia2) picks the right Rust backend.
* Other vbuf backends (mshtml, webKit, lotusNotesRichText, adobeAcrobat) still C++ and would need their own ports OR keep using the C-shim layer to call into the Rust storage.

This is the largest single piece of work in the roadmap and probably the right place to draw the "stop here for now" line on a first pass.

## Architectural decisions captured

* **`matchAttributes`**: Rust-side regex (no shim wrapping). Avoids threading `wregex` through the FFI for the one constant case gecko_ia2 has.
* **vbuf bindings as opaque newtypes**: matches the COM-interface pattern we've used throughout; consistent ergonomics.
* **Phase 5 polymorphism**: keep a small C++ adapter on the first pass; defer the bigger `vbufBase` port until/unless we want to remove that adapter.
* **Group F pure-logic helpers**: stay in C++ until the attribute map representation moves to Rust (probably as part of Phase 4's `fillVBuf` design).

## Open questions

* **Phase 2 crate layout**: keep vbuf bindings under `nvda_ia2`, or split into a new `nvda_vbuf` crate? Probably new crate -- vbufBase is logically separate from IA2 and other backends will want it.
* **Phase 4 sub-plan**: `fillVBuf` is large enough to warrant its own design doc. Will write one when we get there.
* **Phase 6 scope**: do we also commit to porting other vbuf backends (mshtml, webKit, etc.)? If yes, Phase 6 grows to "port everything in vbufBase + every backend that uses it." If no, Phase 6 is just `vbufBase` itself and the existing C++ backends keep their C-shim use.
