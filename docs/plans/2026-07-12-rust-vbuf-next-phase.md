# Next phase for the Rust vbuf migration

**Status:** scoping (2026-07-12). Phase 6e is done, verified, benchmarked,
and optimized. This maps what "the bigger plan" realistically is from here.

## Where we are

- **gecko_ia2** (Firefox / Chrome — the backend that matters) renders into
  and reads out of the Rust `nvda_vbuf::storage::Buffer`, live on
  `direct_rust_storage` = default. Audio + browse mode verified on real pages.
- **Baseline + optimization:** `rust/vbuf_bench` compares Rust storage vs C++
  `VBufStorage_buffer_t` on identical workloads. Rust wins most ops (writes
  1.3–4.6×, reads 1.1–2.6×). `find_node_by_attributes` now caches the compiled
  regex per buffer, flipping the early-hit cases to Rust-faster.
- **Still C++:** the vbufBase (`storage.cpp`, `backend.cpp`, `utils.cpp`) +
  `c_shim.cpp`, used by (a) the four other backends (mshtml, webKit,
  adobeAcrobat, lotusNotesRichText) and (b) gecko's own thin C++ adapter
  `GeckoVBufBackend_t`, which inherits `VBufBackend_t` for the render-thread
  machinery.

## The load-bearing architectural constraint

The 6e design deliberately keeps the `VBufBackend_t` **render-thread machinery**
in C++ — `SetTimer`/`requestUpdate`, the `WH_CALLWNDPROC` + WinEvent destroy
hooks, `execInThread`, `runningBackends`, and the `LockableObject`. Rust decides
*what* to render and *where* to store it; C++ still owns *when* (scheduling) and
thread affinity. Consequences:

- **`gecko_ia2.cpp` cannot be fully deleted.** It IS the required C++
  `VBufBackend_t` polymorphic adapter + render-thread shim. The roadmap's
  "Phase 6f: delete gecko_ia2.cpp" is only reachable by *also* porting the
  render-thread machinery to Rust — a large, low-value task the design defers
  indefinitely (unsafe Win32 FFI for marginal benefit).
- The realistic migration end-state is **Rust storage behind a thin C++
  render-thread adapter per backend**, not zero C++.

## Options for the next phase

### A. Stage E — consolidate the gecko win (small; recommended first)
Prune the vestigial gecko `render()` override and the now-dead feature-off
branches in `nvda_ia2` (`direct_rust_storage` is `default`, so the
`#[cfg(not(feature))]` arms are dead weight). Behaviour-preserving, low risk.
Deliverable: smaller, clearer gecko path; keeps the `--no-default-features`
revert working (decide explicitly whether to keep it as the rollback escape or
drop it). ~1 session.

### B. Retire C++ `storage.cpp` by migrating the other backends (large; the real remaining migration)
To delete `storage.cpp`, every backend must use Rust storage. Two shapes:

- **B1 — flip the c_shim's storage (big-bang "Shape B").** Rewrite `c_shim.cpp`
  so `vbuf_*` operate on a Rust `Box<Buffer>` instead of C++
  `VBufStorage_buffer_t`; the four other backends (which all call the c_shim)
  transparently get Rust storage, their C++ `fillVBuf` unchanged. Also requires
  routing `VBufBackend_t::update()`/`replaceSubtrees` to Rust storage for those
  backends. Regressions would hit Office/PDF/Lotus at once — **high risk**.
- **B2 — per-backend port (repeat the gecko pattern).** Migrate one backend at a
  time. Lower risk per step, but ~4× the gecko effort, against declining
  backends (mshtml/webKit are legacy; adobeAcrobat/lotusNotes are niche).

Both are substantial and target low-traffic backends.

### C. Polish gecko (small–medium)
- The `locate_text_field_at_offset` slotmap-indirection optimization — minor
  (already inverts to Rust-faster at large trees; small-tree loss is the cost of
  the generational-key safety check).
- Broader browse-mode hardening: tables, live regions, ARIA details/errors,
  find-in-page, multi-frame docs — exercise the flipped path against edge cases.

## Recommendation

1. **Stage E now** — quick, in-scope cleanup that finishes the 6e work.
2. Then a **strategic decision** that's the user's to make: is deleting C++
   `storage.cpp` worth migrating four niche/legacy backends (Option B, large +
   risky), or is "gecko-on-Rust + others-on-C++" the pragmatic end-state? The
   design explicitly allows the two storages to coexist indefinitely.
3. If advancing on B, **pilot one low-traffic backend (e.g. adobeAcrobat)** via
   the gecko pattern (B2) to prove the storage generalizes beyond IA2 before
   committing to a big-bang c_shim flip.

`backend.cpp` (render-thread machinery) stays C++ under every option above.
