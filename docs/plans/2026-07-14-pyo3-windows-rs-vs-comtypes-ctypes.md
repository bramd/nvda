# PyO3 + windows-rs vs. comtypes/ctypes: what to expect

**Status:** research memo (2026-07-14). Grounded in the current codebase.
Not a commitment — an assessment to inform future direction.

## Measured results (2026-07-14) — the estimates below were partly wrong

A working benchmark (`rust/uia_bench/`, PyO3 + windows-rs vs. comtypes, both
walking the *same* deterministic test window and reading the same 5 properties
per child, checksums verified identical) measured three sub-paths. Per
property read, N=200 children, Python 3.13:

| path | comtypes | windows-rs (PyO3) | ratio |
|---|---|---|---|
| **live** (uncached; each read marshals to the provider) | ~572 µs | ~535 µs | **1.0×** |
| **cached-walk** (one `FindAllBuildCache` fetch + local reads — NVDA's pattern) | ~409 µs | ~406 µs | **1.0×** |
| **cached-read only** (build the cache first, time *only* local reads) | ~7.0 µs | ~1.0 µs | **7×** |

The headline: **the binding really is ~7× faster in Rust — but only on the
isolated local read, and in every *realistic* UIA pattern that 7× is masked
by UIA's own cross-boundary marshaling**, which both bindings pay identically.

- **Live reads** are ~570 µs *each* — that is the UIA marshaling of
  `GetCurrentPropertyValue` to the provider; comtypes-vs-windows-rs is noise.
- **Cached-walk** (what NVDA actually does) is ~405 ms for 200 elements,
  ~99 % of which is the single `FindAllBuildCache` fetch (still a cross-
  boundary call); the local reads it enables are <1 % → again **1.0×**.
- Only when you fetch once and then read the cache **many** times locally does
  the 7× binding advantage dominate.

**Implications (these correct the estimates further down):**

1. **For NVDA's out-of-process UIA client, a comtypes→windows-rs swap buys
   ≈nothing** at the operation level — UIA marshaling, not the binding, is the
   bottleneck. (And real NVDA reads *cross-process*, where marshaling is even
   heavier than this in-process test window — so this 1.0× is conservative.)
2. **The 7× binding win is real where local reads dominate: in-process COM
   with no marshaling** — which is exactly the vbuf backends' regime (injected
   DLL, in-proc IAccessible/IA2). So the benchmark **validates the vbuf
   porting strategy** and **cautions against** expecting UIA-client wins.
3. The lever for UIA performance is **not the binding language** — it's
   **fetching less across the boundary** (better caching / fewer live reads),
   which NVDA already does and which Rust does not improve.

The comtypes "hot path could win 2–100×" estimate below is only right for the
in-process / read-many-locally case; for typical UIA it's ~1×. Everything
after this section is the pre-measurement reasoning, kept for context.

## The footprint (measured)

- **comtypes: ~100 source files.** Concentrated in the generated COM
  wrappers (`comInterfaces/`, typelib-generated proxies:
  `UIAutomationClient.py`, `IAccessible2Lib.py`, …) and the **hottest code
  in the product** — the accessibility object model
  (`NVDAObjects/UIA/__init__.py`, `NVDAObjects/IAccessible/__init__.py`;
  83 and 96 comtypes/QI/COMError references respectively) plus
  `UIAHandler/` and app modules.
- **ctypes: ~164 source files.** A dedicated `winBindings/` package
  (31 files: user32, kernel32, gdi32, oleacc, mshtml, magnification, …) of
  hand-written Win32 bindings, plus the object model, synth/braille drivers,
  and `hwIo`. Notably `winBindings/` is **2025-dated** — NVDA is already
  consolidating scattered ctypes into a typed, central package, i.e. the
  maintenance burden is recognized and being addressed *within Python*.

These are **two different problems** with different answers.

## comtypes (COM) — the strong case

**Why it's error-prone / slow.** comtypes is pure-Python, typelib-driven,
dynamic COM dispatch. Every property read —
`_getUIACacheablePropertyValue(...)`, `element.CurrentName`,
`GetCurrentPropertyValue(...)` — is a Python-level COM call with argument
marshaling, VARIANT box/unbox, `COMError` handling, and the GIL. NVDA reads
*many* properties per object and walks *trees* of these objects on **every
focus / navigation event**, so per-node comtypes overhead compounds. The
correctness footguns are the usual COM ones (VARIANT/BSTR, refcounts,
apartment/threading — the UIA event-limiter existed precisely because
comtypes-on-the-UIA-thread blocking was a problem).

**What PyO3 + windows-rs gives.** windows-rs COM calls are **direct vtable
calls** (near-zero overhead vs. comtypes' dynamic dispatch), with
**compile-time-correct** signatures generated from Win32 metadata (no
hand-rolled VARIANT/struct mistakes), Rust ownership for refcounts, and
explicit `Send`/`Sync`/`AgileReference` for apartments.

**Performance reality:** the big win appears only when you move a **whole
operation** into Rust — e.g. "walk this UIA subtree, read these 8 properties
per node, return a flat result" done entirely in Rust, crossing the
Python↔Rust boundary **once**. That amortizes the boundary *and* eliminates
per-node comtypes dispatch → plausibly large (this is exactly the shape of
the vbuf backend ports, which won big). A **1:1** replacement (one PyO3 call
per one COM call) wins far less: you trade comtypes dispatch for PyO3
dispatch and still pay the boundary per call. The per-call win (vtable vs.
dynamic dispatch) is real, but the **boundary-amortization** is where it
compounds.

## ctypes (Win32) — the weak case

**Why it's error-prone.** Hand-declared `argtypes`/`restype`/`Structure`s.
A wrong int width, pointer-vs-value, missing `argtypes`, or by-hand struct
layout is a **silent** memory corruption or crash — no compile check.
Callback objects (`WINFUNCTYPE`) must be kept alive by hand.

**But:** Win32 calls themselves are cheap C calls; the ctypes cost is
per-call marshaling, which only bites in tight loops. And NVDA is *already*
reducing the hand-written surface via `winBindings/`.

**What PyO3 + windows-rs gives.** Correct-by-construction Win32
signatures/structs (from metadata) — this deletes the ABI-mismatch bug
class — and memory safety at the boundary. **Performance is usually a wash
per-call** (ctypes overhead ≈ PyO3 overhead); the win is *correctness*, not
speed, unless a hot loop of Win32 calls moves wholesale into Rust.

## Stability — realistic expectation

**Eliminated (compile-time in Rust):** ABI mismatches, struct-layout errors,
VARIANT/BSTR mishandling, many refcount/lifetime bugs. This is the strongest
argument, and it's real.

**NOT eliminated:** the Windows APIs are still `unsafe` (windows-rs calls are
`unsafe fn`); COM apartment/threading complexity is inherent (Rust models it
more explicitly via `Send`/`Sync`/`AgileReference`, but you still have to get
it right — as we did in the UIA port's D2 decision); and **PyO3 has its own
footguns** (GIL discipline, `Py` object lifetimes, exception/panic
translation across FFI). The honest framing: you **concentrate** the risk in
a typed, testable, reviewable Rust layer instead of scattering it across
~260 Python files. Fewer silent-corruption bugs, more caught at compile/test
time — at the cost of a layer that's harder for the Python-centric
contributor base to work on.

## Performance — realistic expectation

- **comtypes-heavy hot paths** (UIA/IAccessible property reads + tree walks):
  potentially **large** wins on the moved portion — *if* whole operations
  move to Rust, not 1:1 calls. Same shape as the vbuf ports.
- **ctypes Win32 calls:** mostly a **wash** per-call; wins only for tight
  loops moved wholesale.
- **The PyO3 boundary isn't free** (~sub-µs per crossing), so granular 1:1
  replacement can be net-neutral or slightly negative. **Coarse-grained**
  "do a whole subtask in Rust" is where it pays.

## The strategic caveats (why not "replace it all")

1. **Don't 1:1-replace.** Swapping every comtypes/ctypes call for a PyO3
   binding is a massive core rewrite, fragments the two COM object models
   (comtypes objects vs. windows-rs objects) with marshaling at every
   boundary, and often wouldn't win.
2. **Object-model impedance.** NVDA represents accessibles as Python
   `NVDAObject`s holding comtypes pointers, woven through the event / speech
   / braille pipeline. You can't move an `NVDAObject` to Rust without moving
   huge swaths of NVDA. Realistic targets are **leaf operations** (a tree
   walk, a text extraction, an event throttle), not the object model itself.
3. **Contributor ecosystem.** Add-ons and contributors are Python; core-in-
   Rust raises the barrier — a genuine strategic cost.
4. **Interop friction.** Passing COM objects across the comtypes↔windows-rs
   boundary works (via raw `IUnknown`), but adds marshaling boilerplate at
   each crossing (the UIA event-limiter shows this). Fine for a handful of
   boundaries; painful if pervasive.

## Recommendation

Keep doing **exactly** what the vbuf backends and the UIA event-limiter did:
target **hot, self-contained, COM/ctypes-heavy subsystems** and move those
whole. Do **not** 1:1-port the `winBindings` layer (low perf win; NVDA is
already improving it in Python) and do **not** try to move the `NVDAObject`
model (too woven into Python).

The next high-value candidates are other **comtypes-heavy hot loops** where
per-node dynamic dispatch dominates: UIA subtree walks / batched property
reads, IAccessible traversal, and `displayModel` text extraction.

**To turn "plausibly large" into a number:** prototype + benchmark **one**
representative comtypes hot path — e.g. a UIA subtree property-batch read,
comtypes-in-Python vs. windows-rs-in-Rust-via-PyO3 — the same way
`vbuf_bench` measured the storage rewrite. That converts this memo's
estimates into a decision-grade measurement before committing to a bigger
push.
