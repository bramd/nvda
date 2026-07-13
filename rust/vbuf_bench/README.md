# vbuf_bench

Criterion micro-benchmarks comparing the **Rust** virtual-buffer storage
(`nvda_vbuf::storage::Buffer`) against the **original C++**
`VBufStorage_buffer_t` (`nvdaHelper/vbufBase/storage.cpp`) on identical
synthetic workloads, **in one process**.

The point is an apples-to-apples baseline: both engines are driven from a
single deterministic op list, so their trees are structurally identical (same
`(docHandle, ID)` node identities, same text, same attributes). Any timing
difference is the implementation, not the workload. Capture the numbers now
and re-run after a change to catch regressions.

## Running

```sh
# from the repo's rust/ workspace dir
cargo bench -p vbuf_bench

# faster, lower-fidelity run (used to capture the baseline below):
cargo bench -p vbuf_bench -- --sample-size 10 --measurement-time 1.0 --warm-up-time 0.5

# a single group, or a single (size/shape):
cargo bench -p vbuf_bench -- get_text_in_range_markup
cargo bench -p vbuf_bench -- large/realistic_mixed
```

HTML reports land in `target/criterion/` (or `build/rust/criterion/` when you
pass `--target-dir build/rust`, as the rest of the repo does).

`cargo` is all you need — the uv Python env is irrelevant here. On Windows the
`build.rs` compiles the C++ with MSVC `cl.exe` (located automatically by the
`cc` crate).

## Regression guard

`regression_check.py` turns this benchmark into a go/no-go regression check
you can run before/after a storage change (or in CI). It runs the full
benchmark, then compares the **Rust-vs-C++ ratio** (`rust_ns / cpp_ns`) of
every `(op, size, shape)` against a committed baseline
(`regression_baseline.json`).

Why the ratio rather than absolute times: both engines are measured in the
*same* run, so the ratio cancels out machine speed and run-to-run noise that
hits both equally. A regression therefore means the **Rust** engine got
slower *relative to the unchanged C++ reference* — the actual "did our
rewrite regress?" signal — and the baseline stays meaningful across machines.

```sh
# from the repo's rust/ dir (needs `cargo` on PATH; no uv/venv needed)
python vbuf_bench/regression_check.py            # run + compare; exit 1 on regression
python vbuf_bench/regression_check.py --update   # (re)capture the committed baseline
python vbuf_bench/regression_check.py --no-run   # compare using the last criterion run
python vbuf_bench/regression_check.py --threshold 15   # tighten from the default 20%
```

It prints a table sorted worst-first (`base r/c`, `now r/c`, `delta%`, flag)
and exits non-zero if any gated op's ratio worsened past the threshold
(default 20%, chosen to absorb the sampling noise the caveats below describe).
`get_text_length` (a ~1 ns O(1) floor marker) is reported but excluded from
the gate. Re-run `--update` intentionally when a change legitimately shifts
the numbers.

This guards the storage engine (the part the port rewrote). The render
(`fillVBuf`) walks a live COM tree from a running app, so it can't be
benchmarked headlessly here; its storage-write cost *is* covered by the
`construct` op with realistic shapes (including `mshtml`).

## How it is wired

* `build.rs` compiles three C++ files into a static lib with the same flags
  scons uses for nvdaHelper (`/std:c++20 /EHsc /DUNICODE /D_UNICODE /DNOMINMAX
  /D_WIN32_WINNT=0x0A00 /DNDEBUG`, plus `/O2` from the release bench profile):
  `nvdaHelper/vbufBase/storage.cpp`, `nvdaHelper/vbufBase/utils.cpp`, and
  `cpp/bench_shim.cpp`. `storage.cpp` is self-contained (only std headers +
  header-only `common/xml.h` + `common/log.h` + `utils.*`); it is compiled with
  `/DLOGLEVEL=60` so every `LOG_*` macro becomes a no-op and no logging runtime
  is needed.
* `cpp/bench_shim.cpp` is a thin `extern "C"` surface over the buffer's public
  storage API. It deliberately does **not** reuse `nvdaHelper/vbufBase/c_shim.cpp`
  (that shim pulls in `VBufBackend_t` → `backend.cpp` → RPC/COM).
* `benches/storage.rs` calls `nvda_vbuf::storage::Buffer`'s public methods
  directly (no FFI hop) for the Rust side, and the `vbench_*` shim for the C++
  side. `nvda_vbuf` is a **dev-dependency with the `test_stubs` feature** so the
  crate's default C-shim wrapper code links (the benchmark never calls those
  stubs; it uses `Buffer` directly).

This crate is a standalone workspace member that the scons production DLL build
never compiles — that build only builds `--package nvda_ia2 --package
nvda_input_hooks` (see `nvdaHelper/remote/sconscript`). Keeping the C++-compiling
`build.rs` out of that list is what prevents duplicate storage symbols in the DLL.

## Workloads

Four **shapes**, each generated at three **sizes** (`small` ≈ 200 nodes,
`medium` ≈ 2 000, `large` ≈ 10 000) from a fixed-seed xorshift RNG:

* **`wide_shallow`** — a root with many block control children, each holding one
  text node. Every 10th child is a heading (with a `level`); all carry `role` +
  `class` attributes.
* **`deep_nested`** — a long nested control spine (each level a block control
  with a text run), widened with extra leaf link+text pairs to hit the node
  budget. Spine depth is capped (100 / 700 / 1000) so the recursive
  `getTextInRange` / `calculateOffsetInTree` stay within the main thread's ~1 MB
  Windows stack in both engines.
* **`realistic_mixed`** — headings / paragraphs / links / lists at a few control
  levels, with a realistic distribution (~12 % headings, ~50 % paragraphs, ~20 %
  lists, ~18 % links), a couple of attributes per control node, and realistic
  word-length text runs.
* **`mshtml`** — the same web-page structure as `realistic_mixed` (plus small
  tables and inline strong runs), but with the per-control-node **attribute
  density a real MSHTML (Trident) render emits**: `IHTMLDOMNode::nodeName`, a
  numeric `IAccessible::role`, one or more `IAccessible::state_N`, `language`,
  and `formatState` (~6–9 attributes vs `realistic_mixed`'s 2–3). All vbuf
  backends — gecko, acrobat, mshtml — render into the *same*
  `nvda_vbuf::storage::Buffer`, so the only storage cost specific to the MSHTML
  backend is this attribute count, which stresses the per-node attribute map.
  (Text-node attributes are omitted — `BuildOp::Text` carries none and control
  nodes dominate the cost.)

## Benchmark groups

Each group runs `rust` and `cpp` side by side per `(size, shape)`.

1. **`construct`** — build the whole tree from scratch (the fillVBuf write path);
   timed.
2. **`get_text_in_range_plain`** — `getTextInRange(0, len, markup=false)` over the
   full buffer.
3. **`get_text_in_range_markup`** — same with `markup=true` (XML tag generation).
4. **`get_text_length`** — the O(1) root-length read.
5. **`find_node_by_attributes`** — a "find next heading" quick-nav search forward
   from offset 0. `attribs` / `regexp` are built exactly as
   `source/virtualBuffers/__init__.py::_prepareForFindByAttributes` builds them
   for `{"role": ["heading"]}` → `attribs = "role"`, `regexp = "role:(?:heading;)"`.
6. **`locate_text_field_at_offset`** — a fixed set of 100 pseudo-random offsets
   (same offsets for both engines).
7. **`get_line_offsets`** — the same offset set, `maxLineLength = 100`,
   `useScreenLayout = true`.
8. **`replace_subtrees`** — pick a mid-tree control node, build a small
   replacement subtree in a temp buffer, and merge it in. Uses `iter_batched`
   (`SmallInput`) so the fresh-buffer setup isn't timed.

To keep the FFI boundary allocation-free and symmetric, the C++ `getTextInRange`
shim builds the (marked-up) `std::wstring` and returns only its length; the Rust
side likewise fills a `Vec<u16>` and reports `len()`. We measure the work, not
the marshalling.

## Baseline results

Machine: `x86_64-pc-windows-msvc`, MSVC 14.44, rustc 1.94, release/`/O2`.
Captured with `--sample-size 10 --measurement-time 1.0 --warm-up-time 0.5`
(a quick, slightly noisy run — treat ratios as directional, not exact). Times
are criterion medians. **C++ / Rust > 1 means Rust is faster.**

### `construct`

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 27.69 µs | 101.97 µs | 3.68× |
| small | deep_nested | 53.86 µs | 112.86 µs | 2.10× |
| small | realistic_mixed | 41.54 µs | 114.42 µs | 2.75× |
| medium | wide_shallow | 484.90 µs | 1.06 ms | 2.19× |
| medium | deep_nested | 1.93 ms | 2.49 ms | 1.29× |
| medium | realistic_mixed | 481.23 µs | 1.16 ms | 2.41× |
| large | wide_shallow | 4.55 ms | 6.96 ms | 1.53× |
| large | deep_nested | 12.55 ms | 24.58 ms | 1.96× |
| large | realistic_mixed | 4.98 ms | 9.34 ms | 1.87× |

### `get_text_length`

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 1.4 ns | 1.7 ns | 1.22× |
| large | realistic_mixed | 1.3 ns | 1.8 ns | 1.32× |

(O(1) in both — a root-length field read. All 9 cells ~1.3–1.8 ns; both are
effectively free. Full matrix omitted.)

### `get_text_in_range_plain`

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 2.55 µs | 2.92 µs | 1.14× |
| small | deep_nested | 2.11 µs | 2.33 µs | 1.11× |
| small | realistic_mixed | 2.00 µs | 2.33 µs | 1.17× |
| medium | wide_shallow | 24.85 µs | 31.47 µs | 1.27× |
| medium | deep_nested | 17.94 µs | 21.63 µs | 1.21× |
| medium | realistic_mixed | 18.93 µs | 28.56 µs | 1.51× |
| large | wide_shallow | 117.69 µs | 180.60 µs | 1.53× |
| large | deep_nested | 99.15 µs | 157.61 µs | 1.59× |
| large | realistic_mixed | 122.82 µs | 199.34 µs | 1.62× |

### `get_text_in_range_markup`

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 201.12 µs | 760.29 µs | 3.78× |
| small | deep_nested | 155.77 µs | 722.71 µs | 4.64× |
| small | realistic_mixed | 190.63 µs | 783.87 µs | 4.11× |
| medium | wide_shallow | 6.51 ms | 12.81 ms | 1.97× |
| medium | deep_nested | 1.85 ms | 7.76 ms | 4.19× |
| medium | realistic_mixed | 3.33 ms | 9.29 ms | 2.79× |
| large | wide_shallow | 136.00 ms | 168.71 ms | 1.24× |
| large | deep_nested | 11.90 ms | 41.09 ms | 3.45× |
| large | realistic_mixed | 44.72 ms | 79.21 ms | 1.77× |

### `find_node_by_attributes` (with per-buffer regex cache)

`Buffer::find_node_by_attributes` now caches the compiled `Regex` keyed by
the raw pattern (quick-nav reissues the same pattern every keypress), so the
~20 µs `Regex::new` is paid once instead of per call. That flipped 5 of the 6
shapes where Rust was slower to Rust-faster. The pre-cache Rust numbers are in
the right column for reference.

| size | shape | Rust (cached) | C++ | C++ / Rust | (Rust pre-cache) |
|---|---|--:|--:|--:|--:|
| small | wide_shallow | 7.79 µs | 20.71 µs | 2.66× | 27.26 µs |
| small | deep_nested | 14.12 µs | 84.88 µs | 6.01× | 29.50 µs |
| small | realistic_mixed | 10.05 µs | 13.86 µs | 1.38× | 33.32 µs |
| medium | wide_shallow | 11.73 µs | 20.82 µs | 1.77× | 25.62 µs |
| medium | deep_nested | 239.36 µs | 1.18 ms | 4.92× | 252.59 µs |
| medium | realistic_mixed | 7.81 µs | 3.29 µs | 0.42× **Rust slower** | 25.32 µs |
| large | wide_shallow | 15.06 µs | 21.50 µs | 1.43× | 30.20 µs |
| large | deep_nested | 1.88 ms | 10.10 ms | 5.37× | 2.58 ms |
| large | realistic_mixed | 18.57 µs | 50.53 µs | 2.72× | 30.74 µs |

The lone remaining Rust-slower case (`medium/realistic_mixed`) is a tiny
early-hit scan now bounded by regex *match* throughput on a handful of nodes,
not compile cost; it improved 3× from the pre-cache number.

### `locate_text_field_at_offset` (100 offsets/iter)

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 16.06 µs | 7.42 µs | 0.46× **Rust slower** |
| small | deep_nested | 16.19 µs | 8.11 µs | 0.50× **Rust slower** |
| small | realistic_mixed | 8.79 µs | 3.32 µs | 0.38× **Rust slower** |
| medium | wide_shallow | 199.90 µs | 190.68 µs | 0.95× **Rust slower** |
| medium | deep_nested | 160.12 µs | 114.14 µs | 0.71× **Rust slower** |
| medium | realistic_mixed | 105.25 µs | 49.70 µs | 0.47× **Rust slower** |
| large | wide_shallow | 1.01 ms | 1.25 ms | 1.24× |
| large | deep_nested | 212.52 µs | 298.71 µs | 1.41× |
| large | realistic_mixed | 644.99 µs | 654.27 µs | 1.01× ~tie |

### `get_line_offsets` (100 offsets/iter)

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 42.57 µs | 92.22 µs | 2.17× |
| small | deep_nested | 37.71 µs | 67.86 µs | 1.80× |
| small | realistic_mixed | 64.62 µs | 168.51 µs | 2.61× |
| medium | wide_shallow | 277.29 µs | 307.73 µs | 1.11× |
| medium | deep_nested | 148.87 µs | 205.34 µs | 1.38× |
| medium | realistic_mixed | 178.90 µs | 322.64 µs | 1.80× |
| large | wide_shallow | 923.80 µs | 1.35 ms | 1.46× |
| large | deep_nested | 251.15 µs | 480.38 µs | 1.91× |
| large | realistic_mixed | 729.42 µs | 874.16 µs | 1.20× |

### `replace_subtrees`

| size | shape | Rust | C++ | C++ / Rust |
|---|---|--:|--:|--:|
| small | wide_shallow | 2.34 µs | 5.85 µs | 2.50× |
| small | deep_nested | 25.23 µs | 49.93 µs | 1.98× |
| small | realistic_mixed | 4.06 µs | 5.31 µs | 1.31× |
| medium | wide_shallow | 3.98 µs | 7.23 µs | 1.82× |
| medium | deep_nested | 146.13 µs | 421.40 µs | 2.88× |
| medium | realistic_mixed | 3.82 µs | 9.75 µs | 2.55× |
| large | wide_shallow | 7.14 µs | 19.06 µs | 2.67× |
| large | deep_nested | 1.05 ms | 1.91 ms | 1.82× |
| large | realistic_mixed | 6.99 µs | 13.32 µs | 1.90× |

## Reading the results

**Headline:** Rust wins most groups, often by 1.2–4×. The write paths
(`construct`, `replace_subtrees`) are consistently ~1.3–3.7× faster — the
slotmap arena allocates nodes in bulk-ish slots rather than one `new` per node,
and the C++ side pays for `std::set` / `std::map` bookkeeping on every insert.
Markup generation is the biggest Rust win (up to ~4.6×): C++ builds the XML by
repeatedly appending to `std::wstring` through several virtual `generate*` calls
per node, versus Rust appending to a `Vec<u16>`.

**Where Rust is slower — two clear, explainable patterns:**

* **`locate_text_field_at_offset` at small/medium sizes (~0.4–0.9×).** This is
  pure pointer-chasing down the child chain. C++ follows raw `firstChild` /
  `next` pointers; Rust does a `slotmap` generational-key lookup (bounds +
  generation check, then index) on every hop. That per-node indirection is the
  cost of memory-safety-by-construction, and it dominates when the trees are
  small and cache-resident. It **inverts at `large`** (Rust 1.0–1.4×): the
  slotmap's contiguous arena is more cache-friendly than C++'s individually
  `new`'d nodes scattered across the heap, so once the working set stops fitting
  in cache the arena layout wins back the indirection cost.

* **`find_node_by_attributes` (was ~0.1–0.9× when hit early; now cached).** The
  Rust `regex` crate has an expensive one-time `Regex::new` compile (~20 µs) but
  fast matching; C++ `std::wregex` compiles cheaply but matches slowly. Early-hit
  shapes (the first heading is a handful of nodes in) used to be dominated by the
  Rust compile cost, so C++ won. `Buffer::find_node_by_attributes` now caches the
  compiled regex keyed by the raw pattern; quick-nav reissues the same pattern on
  every keypress, so the compile is paid once and every later search is a map
  lookup + cheap Arc-backed `Regex` clone. That flips the early-hit shapes to
  Rust-faster and keeps the long-scan wins (`deep_nested` still 5–6× as Rust's
  faster per-node matching dominates). One tiny early-hit case
  (`medium/realistic_mixed`, ~8 µs vs C++ ~3 µs) remains Rust-slower — now bounded
  by match throughput on a very short scan, not compile. C++ still recompiles per
  call; this cache is a pure win on top of behavioural parity.

`get_text_length` is O(1) in both and effectively free (~1.5 ns); it exists only
as a floor/sanity marker.

### `mshtml` shape — all ops (attribute-dense, added 2026-07-13)

The MSHTML backend renders into the same `storage::Buffer` as gecko/acrobat, so
this shape is a *workload*, not a separate engine. Its point is the per-node
attribute density MSHTML emits (~6–9 attrs/node), which stresses the attribute
map. Median of a `--sample-size 10` run:

| op | size | Rust | C++ | C++ / Rust |
| --- | --- | --- | --- | --- |
| construct | small | 60.3 µs | 203.8 µs | 3.38× |
| construct | medium | 1.19 ms | 2.25 ms | 1.90× |
| construct | large | 10.94 ms | 17.34 ms | 1.59× |
| get_text_in_range_plain | small | 1.56 µs | 1.96 µs | 1.26× |
| get_text_in_range_plain | medium | 16.18 µs | 20.21 µs | 1.25× |
| get_text_in_range_plain | large | 93.59 µs | 166.43 µs | 1.78× |
| get_text_in_range_markup | small | 184.8 µs | 806.1 µs | 4.36× |
| get_text_in_range_markup | medium | 2.93 ms | 9.79 ms | 3.34× |
| get_text_in_range_markup | large | 31.14 ms | 74.58 ms | 2.39× |
| find_node_by_attributes | small | 33.0 µs | 143.5 µs | 4.35× |
| find_node_by_attributes | medium | 12.35 µs | 29.29 µs | 2.37× |
| find_node_by_attributes | large | 9.97 µs | 13.83 µs | 1.39× |
| locate_text_field_at_offset | small | 3.85 µs | 1.54 µs | **0.40×** |
| locate_text_field_at_offset | medium | 51.32 µs | 37.51 µs | **0.73×** |
| locate_text_field_at_offset | large | 385.0 µs | 384.4 µs | 1.00× |
| get_line_offsets | small | 85.3 µs | 193.6 µs | 2.27× |
| get_line_offsets | medium | 118.3 µs | 211.7 µs | 1.79× |
| get_line_offsets | large | 454.8 µs | 569.2 µs | 1.25× |
| replace_subtrees | small | 4.24 µs | 8.03 µs | 1.89× |
| replace_subtrees | medium | 5.37 µs | 10.73 µs | 2.00× |
| replace_subtrees | large | 9.52 µs | 12.05 µs | 1.27× |
| get_text_length | all | ~1.3 ns | ~2.0 ns | ~1.5× |

Takeaway: the attribute density **amplifies** Rust's two biggest structural wins
— markup serialization (`get_text_in_range_markup`, up to 4.4×, since every
attribute is emitted into the `<control>` tag) and `find_node_by_attributes`
(up to 4.3×) — and makes `construct` cheaper relative to C++ (more attribute-map
inserts, where Rust wins). The one Rust-slower op is
`locate_text_field_at_offset` on small/medium trees (the slotmap generational-key
lookup vs a raw C++ pointer-follow), matching the existing baseline finding — it
reaches parity at `large` where the contiguous arena wins back the indirection.

### Caveats

* The baseline was captured at `--sample-size 10` for speed; a handful of cells
  had 10–30 % outliers. Treat sub-1.1× ratios as "roughly tied". Re-run with the
  default sample size before drawing fine conclusions.
* `deep_nested`'s spine depth is capped for stack safety, so its "large" node
  count is reached partly by breadth; its depth (≤1000) is still deep enough to
  exercise the recursive text/offset walks.
* Because nodes are inserted first-child-first, `deep_nested`'s rendered text
  order is deepest-spine-first. This does not change tree depth or the
  Rust-vs-C++ comparison (both build the identical tree from the same op list;
  the `text_length` equality assertion in `build_cases` verifies structural
  parity for every case).

## Ops not wired up

None. Every op named in the design is benchmarked. `replace_subtrees` is
exercised through a one-entry map (`vbench_replace_subtrees_one` /
`Buffer::replace_subtrees`); reference-node resolution across buffers is not
separately benchmarked (the synthetic temp subtree contains no reference nodes),
but the merge/remove/re-anchor path it drives is the same.
