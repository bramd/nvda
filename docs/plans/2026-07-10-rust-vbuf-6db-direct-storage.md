# Phase 6d-b: route nvda_vbuf's newtypes to Rust storage

**Status:** implemented (2026-07-10). Behind a default-off Cargo
feature; no caller-visible change.

Phase 6d-a added a parallel `nvda_vbuf_*` `extern "C"` API over the
Rust `storage::Buffer` (see `extern_api.rs`). This phase makes the
existing newtype wrappers in `lib.rs` (`VbufBuffer` / `VbufFieldNode`
/ `VbufControlFieldNode` / `VbufBackend`) able to call that Rust
storage *directly* -- no FFI hop -- under a feature flag, so we can
A/B the two storage backends before committing gecko_ia2 to Rust in
Phase 6e.

## The feature flag

`direct_rust_storage`, declared in `nvda_vbuf/Cargo.toml`, **default
off**. Everything compiles and every test passes in both
configurations.

* **OFF (today's behaviour):** the newtypes wrap `*mut c_void`; every
  method forwards to the `vbuf_*` externs defined in
  `nvdaHelper/vbufBase/c_shim.cpp`. Byte-for-byte the pre-6d-b code.
* **ON:** `VbufBuffer` wraps `*mut storage::Buffer` and the methods
  call `Buffer` methods on it. No C++ involvement for buffer/node ops.

`nvda_ia2` never enables the feature (its dependency has no features;
its dev-dependency enables only `test_stubs`), so its build and tests
are entirely unaffected and its use of the newtypes is unchanged.

## Handle representation (the crux)

C++ node identity is a raw `VBufStorage_fieldNode_t*`. Rust node
identity is a `slotmap` `NodeKey` -- a `u64` -- which **cannot** be
packed into a `*mut c_void`: on x86 pointers are 32-bit but a key is
64-bit. So under the feature a node handle can't be a bare pointer.

The buffer-less method signatures (`node.get_length()`,
`node.add_attribute(..)`) must keep working, so a node handle has to
carry enough to reach its buffer. Under the feature the three node
newtypes therefore wrap a small `Copy` back-pointer struct:

```rust
struct NodeRef { buffer: *mut storage::Buffer, key: NodeKey }
```

Under the feature OFF they stay `*mut c_void`. The struct definitions
are `cfg`-switched; the public method **surface is identical** in both
configs (same signatures, same docs), with per-method `cfg`'d bodies.
`as_field_node` upcasts by copying the inner handle in both configs.

## The `VbufBackend` carve-out

`VbufBackend` wraps the C++ `VBufBackend_t*` and stays routed to the
C-shim in **both** configs -- backend render-thread orchestration
moves to Rust in Phase 6e, not here. It always wraps `*mut c_void`.

Its node-free methods (`root_doc_handle`, `root_id`, `clear_buffer`,
`force_update`, `pending_invalid_subtrees_empty`) are coherent in both
configs and stay as-is.

Three methods traffic in Rust-vs-C++ identity and become **incoherent**
under the feature:

* `as_buffer()` -- upcasts the C++ backend pointer to `VbufBuffer`,
  but under the feature `VbufBuffer` is a `*mut storage::Buffer`; the
  C++ backend is not a Rust `Buffer`.
* `invalidate_subtree(node)` / `reuse_existing_node(..) -> node` --
  take/return node handles, which are Rust keys under the feature but
  C++ pointers on the backend side.

Rather than let these compile into a runtime lie, they are
`cfg(not(direct_rust_storage))` -- **removed under the feature**. Any
code that both enables the feature and calls them fails to *compile*,
which is the honest signal: the backend-as-buffer / backend-node path
is unusable until Phase 6e re-homes backend orchestration onto the
Rust `Buffer`. `nvda_ia2` (feature off) keeps all three.

## Semantics parity and divergences

Under the feature, behaviour matches what the C-shim would do,
including null/None handling: `add_*` return `None` on failure,
`get_attribute` returns `None` when absent. Where the C++ dereferences
an invalid/stale handle (UB / crash), Rust is **stricter and cannot
crash**: a stale `NodeKey` fails the slotmap lookup, so getters return
`0` / `None` / `false` and setters are no-ops. Each such method
documents the divergence inline.

## What becomes testable

With the feature ON the wrapper layer runs against the *real* Rust
storage (today `test_stubs` only lets dependents link against
panicking stubs). New `cfg(all(test, feature = "direct_rust_storage"))`
tests in `lib.rs` exercise the surface end-to-end: create a buffer via
`Box::into_raw`, wrap it, add control + text nodes, round-trip
attributes, check text length / `is_descendant` / `is_node_in_buffer`
/ content queries, and re-`Box` the pointer to free without leaks.

## What Phase 6e still owes

* Re-home `VBufBackend_t`'s node-level and buffer-upcast operations
  (`as_buffer`, `invalidate_subtree`, `reuse_existing_node`) onto the
  Rust `Buffer` so they are coherent under the feature.
* Give gecko_ia2 ownership of a Rust `Buffer` and switch its render
  path onto it, turning the feature on for real.
* The Win32 render-thread machinery (timer, hooks, `execInThread`)
  stays C++ regardless, per the 6d integration plan.
