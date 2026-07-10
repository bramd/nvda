# VBufRemote → Rust storage coverage audit

**Status:** audit (2026-07-10). One gap found and ported; rest already
covered.

`nvdaHelper/remote/vbufRemote.cpp` is the complete Python-facing vbuf
RPC surface (`VBufRemote_*`, driven from `source/virtualBuffers/` via
`localLib`). Each RPC casts the opaque handle to a `VBufBackend_t*` and
calls a method on it. Because `VBufBackend_t` **is-a**
`VBufStorage_buffer_t`, every RPC below except buffer lifecycle bottoms
out in a `VBufStorage_buffer_t` method that the Rust `storage::Buffer`
already ports — with two exceptions kept deliberately C++-side (buffer
create/destroy + RPC rundown), which belong to the render-thread /
backend-lifecycle machinery that Phase 6e re-homes, not to pure
storage.

This audit checks every RPC against `rust/nvda_vbuf/src/storage/mod.rs`
(the ported `Buffer`) and its parallel `nvda_vbuf_*` C ABI in
`rust/nvda_vbuf/src/extern_api.rs`.

## Coverage table

| VBufRemote fn | C++ target (VBufStorage_buffer_t unless noted) | Rust `Buffer` method | `extern_api.rs` | Status |
| --- | --- | --- | --- | --- |
| `createBuffer` | backend factory + `VBufBackend_t::initialize` | — | — | **C++-retained** (backend/render machinery) |
| `destroyBuffer` | `VBufBackend_t::terminate` / `lock` / `destroy` + `FreeLibrary` (the `#ifndef NDEBUG Beep(4000,80)` lives here) | — | `nvda_vbuf_buffer_destroy` covers the *pure buffer* free; RPC-level backend teardown stays C++ | **C++-retained** (backend lifecycle) |
| `bufferHandle_t_rundown` | RPC cleanup → `destroyBuffer` | — | — | **C++-retained** (RPC/COM machinery) |
| `getFieldNodeOffsets` | `getFieldNodeOffsets` | `field_node_offsets` | `nvda_vbuf_buffer_field_node_offsets` | Covered |
| `isFieldNodeAtOffset` | `isFieldNodeAtOffset` | `is_field_node_at_offset` | `nvda_vbuf_buffer_is_field_node_at_offset` | Covered |
| `locateTextFieldNodeAtOffset` | `locateTextFieldNodeAtOffset` (storage.cpp:865) | `buffer_locate_text_field_node_at_offset` | `nvda_vbuf_buffer_locate_text_field_node_at_offset` | **Ported this pass** |
| `locateControlFieldNodeAtOffset` | `locateControlFieldNodeAtOffset` | `locate_control_field_node_at_offset` | `nvda_vbuf_buffer_locate_control_field_node_at_offset` | Covered |
| `getControlFieldNodeWithIdentifier` | `getControlFieldNodeWithIdentifier` | `get_control_field_node_with_identifier` | `nvda_vbuf_buffer_get_control_field_node_with_identifier` | Covered |
| `getIdentifierFromControlFieldNode` | `getIdentifierFromControlFieldNode` | `identifier_of_control_field_node` | `nvda_vbuf_node_identifier` | Covered |
| `findNodeByAttributes` | `findNodeByAttributes` | `find_node_by_attributes` | `nvda_vbuf_buffer_find_node_by_attributes` | Covered (prior pass) |
| `getSelectionOffsets` | `getSelectionOffsets` | `selection_offsets` | `nvda_vbuf_buffer_get_selection_offsets` | Covered |
| `setSelectionOffsets` | `setSelectionOffsets` | `set_selection_offsets` | `nvda_vbuf_buffer_set_selection_offsets` | Covered |
| `getTextLength` | `getTextLength` | `text_length` | `nvda_vbuf_buffer_text_length` | Covered |
| `getTextInRange` | `getTextInRange` (+ `useMarkup`) | `buffer_get_text_in_range` | `nvda_vbuf_buffer_get_text_in_range` | Covered |
| `getLineOffsets` | `getLineOffsets` | `line_offsets` | `nvda_vbuf_buffer_line_offsets` | Covered |

### Additional `extern_api` surface beyond VBufRemote

`extern_api.rs` also exposes storage operations the C-shim needs that
are *not* individual VBufRemote RPCs (they are driven by `fillVBuf` /
backend rendering, which the Python side reaches through
create/update rather than a dedicated RPC): node insertion
(`add_control_field_node` / `add_text_field_node` /
`add_reference_node`), attribute get/set, block/hidden/rerender flags,
`has_useful_content`, `content_matches_string`, `is_descendant_node`,
`is_node_in_buffer`, `clear`, `has_content`, `invalidate_subtree`,
`pending_invalid_subtrees_empty`, and `reuse_existing_node`. These were
in place before this audit.

## What stays C++-side, and why

* **`createBuffer` / `destroyBuffer` / `bufferHandle_t_rundown`.** These
  are backend lifecycle + RPC rundown, not storage. They construct a
  concrete `VBufBackend_t` from the factory map, run
  `initialize()`/`terminate()`, take the render `lock`, and manage the
  `nvdaHelperRemote.dll` module refcount (`GetModuleHandleEx` /
  `FreeLibrary`). Per
  `docs/plans/2026-05-07-rust-vbuf-integration.md`, the render thread,
  update scheduling, and backend orchestration deliberately remain C++
  through Phase 6e; the pure-buffer analogue
  (`nvda_vbuf_buffer_create` / `_destroy`) already exists for the Rust
  `Box<Buffer>`, so no storage work is missing here.
* The **`lock.acquire()` / `lock.release()`** wrapping every RPC is
  backend-thread synchronization, not a storage operation; the Rust
  extern API documents the same single-thread affinity contract
  instead of owning a lock.

## The one gap ported this pass: `locateTextFieldNodeAtOffset`

`VBuf_locateTextFieldNodeAtOffset` is live: `virtualBuffers/__init__.py`
uses it for `UNIT_FORMATFIELD` offset expansion. Its C++ target is the
**buffer-level** `VBufStorage_buffer_t::locateTextFieldNodeAtOffset(offset,
nodeStartOffset, nodeEndOffset)` (storage.cpp:865) — distinct from the
recursive `VBufStorage_fieldNode_t::locateTextFieldNodeAtOffset`, which
the Rust port already had as the private helper
`Buffer::locate_text_field_node_at_offset(key, offset)`.

The Rust port was missing the buffer-entry wrapper: validate `offset`
against the root length, walk from the root, and return the text node
plus its buffer-absolute `(start, end)`. Added:

* `storage::LocateTextFieldResult { node, start, end }` (mirrors
  `LocateControlFieldResult`, minus the identifier text nodes lack).
* `Buffer::buffer_locate_text_field_node_at_offset(offset)` — same
  `buffer_*`/bare naming split already used by `buffer_get_text_in_range`
  vs `get_text_in_range`. Returns `None` for an empty buffer or an
  out-of-range offset (`offset < 0 || offset >= getTextLength()`),
  matching the C++ guard. Where the C++ would `nhAssert` on an internal
  failure to locate under an in-range offset, Rust returns `None`
  (documented divergence).
* `extern_api::nvda_vbuf_buffer_locate_text_field_node_at_offset` —
  returns the node's `u64` FFI key (`0` on failure) and writes
  `(start, end)` to the OUT params, parallel to
  `nvda_vbuf_buffer_locate_control_field_node_at_offset`. On failure the
  OUT params are left untouched.
* Keepalive entry in `rust/nvda_ia2/src/vbuf_keepalive.rs`
  (table 37 → 38, both assertions bumped).

Tests: 1 storage test
(`buffer_locate_text_field_node_at_offset_returns_node_and_range` —
covers in-range hits across two text nodes at different tree depths,
empty buffer, and both out-of-range directions) and 1 extern_api test
(`locate_text_field_at_offset_writes_out_params` — buffer-absolute
offsets, OUT-param untouched on miss).

## Verification

From `rust/`:

* `cargo test -p nvda_vbuf` — 105 passed.
* `cargo test -p nvda_vbuf --features direct_rust_storage` — 111 passed.
* `cargo test -p nvda_ia2` — 78 passed (keepalive count = 38).
* `cargo build -p nvda_ia2` — clean, no warnings.
