# Porting the MSHTML vbuf backend to Rust

**Status:** design (2026-07-12). Not started. Third backend after
gecko_ia2 and adobeAcrobat. MSHTML (Trident) is the most-used of the
remaining legacy backends. Follows the proven per-backend pattern; the
shared `nvda_vbuf::backend::run_raw_update` orchestration is reused.

## Can it be tested on Windows 11?

Yes. IE is retired but the Trident/MSHTML engine ships with Win 11, and
NVDA's MSHTML backend attaches wherever an MSHTML document appears:

- **Edge IE Mode** — the most representative, highest-traffic path.
- **HTML Help (`.chm`)** — the content pane is an embedded MSHTML control.
- **HTA apps** (`mshta.exe`) — still run and render via MSHTML.

Any of these exercises `virtualBuffers/MSHTML.py` + the C++
`MshtmlVBufBackend_t`. A small hand-written `.hta` or `.chm` is the
easiest deterministic smoke-test fixture.

## What's reused (proven on gecko + acrobat)

- `nvda_vbuf::storage::Buffer`, the backend-agnostic vbufRemote read
  routing (`getRustStorageBuffer()`), and `run_raw_update`.
- The **aggregate-staticlib** trick: a new `nvda_mshtml` crate rides
  inside `nvda_ia2.lib` (force-linked via `extern crate`) so `nvda_vbuf`
  is bundled once — no duplicate `#[no_mangle]` symbols.
- The thin-C++-adapter + flip shape (override `update()` /
  `getRustStorageBuffer()`, route the change machinery's storage tail to
  Rust, keep the render-thread machinery C++).

## Why MSHTML is bigger than Acrobat (≈2–3×)

1. **No windows-rs bindings.** windows-rs 0.58 has no `Win32_Web_MsHtml`
   module, so all ~15 MSHTML DOM interfaces are hand-rolled (Acrobat's
   `IPDDom*` were also hand-rolled, but fewer/simpler). Mitigation: nearly
   every MSHTML interface derives *directly* from `IDispatch` and is
   obtained by its own `QueryInterface` — they don't inherit each other —
   so **trailing-truncation is safe** (declare each interface's vtable
   only up to the last method we call). The Acrobat "full-vtable-for-a-
   base" trap only applies to `IHTMLDocument2 : IHTMLDocument` (small
   base). Offsets come from the SDK `MsHTML.h` `*Vtbl` structs (Windows
   Kits `.../um/MsHTML.h`) — ground truth, but a per-method counting task
   and the crash-prone part (a wrong offset = call the wrong fn ptr = AV,
   exactly the Acrobat `IPDDomElement` crash).
2. **`fillVBuf` is ~600 lines** (mshtml.cpp:773) with more features than
   Acrobat: table info, list-item indices, preformatted-text handling,
   "atomic" nodes, skip-text, and new-subtree tracking. Plus a `node.cpp`
   (~513 lines) helper module.
3. **Live-update machinery is a COM sink, not a WinEvent hook.**
   `node.cpp`'s `CDispatchChangeSink : IDispatch` is a COM object NVDA
   *implements* and registers per element to receive DOM-mutation
   notifications. Richer than Acrobat's winEvent hook.

## Binding surface (Stage 1)

New crate `rust/nvda_mshtml`, module `interfaces.rs`, mirroring
`nvda_acrobat::interfaces` (`define_interface!` + hand-rolled `_Vtbl`,
`usize` placeholders, offsets from `MsHTML.h`). Interfaces + the methods
the C++ backend actually calls (all `: IDispatch` unless noted):

| Interface | Methods used |
|---|---|
| `IHTMLDOMNode` | `get_nodeName`, `get_nodeType`, `get_attributes`, `get_childNodes`, `get_ownerDocument` |
| `IHTMLDOMNode2` | `get_ownerDocument` (as needed) |
| `IHTMLDOMTextNode` | `get_data` |
| `IHTMLElement` | `get_tagName`, `get_parentElement`, `getAttribute`, `getElementsByTagName`, `get_currentStyle` *(deep slot)* |
| `IHTMLElement2` | `get_currentStyle` / scroll + client-rect getters |
| `IHTMLElement3` | `get_isContentEditable` |
| `IHTMLUniqueName` | `get_uniqueNumber` |
| `IHTMLCurrentStyle` | `get_display`, `get_visibility`, `get_textDecoration`, `get_listStyleType`, `get_fontWeight`, `get_fontStyle` |
| `IHTMLDocument2` *( : IHTMLDocument )* | `get_body` |
| `IHTMLDocument3` | `getElementById` |
| `IHTMLAttributeCollection2` / `IHTMLDOMAttribute` | attribute enumeration |
| `IMarkupServices` / `IMarkupContainer` / `IMarkupPointer` | `CurrentScope`, pointer navigation |

Exact method sets + offsets are finalised while writing the bindings
(read each `*Vtbl` struct in `MsHTML.h`). Each interface gets a
`vtable_slot_counts`-style size test (the guard that would have caught the
Acrobat crash).

## Staged plan (mirrors acrobat)

- **Stage 1 — bindings.** `nvda_mshtml` crate + `interfaces.rs`. IID +
  vtable-size tests. Biggest single chunk; do it carefully.
- **Stage 2 — fillVBuf port.** Port the ~600-line render + node.cpp
  helpers into `nvda_mshtml::fill_vbuf`, driving the bindings, into a Rust
  `storage::Buffer`. Table/list/preformatted/atomic-node features.
- **Stage 3 — backend adapter + change sink.** `MshtmlBackendState` +
  `mshtml_backend_*` externs (create/destroy/get_buffer/clear/update/
  invalidate). Keep `CDispatchChangeSink` in C++ but route its storage
  tail (node lookup + `invalidateSubtree`) to Rust. Reduce `mshtml.cpp`
  to the thin adapter; aggregate `nvda_mshtml` into `nvda_ia2.lib`.
- **Stage 4 — flip + smoke test.** Override `update()` /
  `getRustStorageBuffer()`. Smoke-test via Edge IE Mode / a `.chm` /
  a `.hta`: linear reading, headings/links quick-nav, a table, a form,
  and a dynamically-updating page (to exercise the change sink).

## Risks

- **Vtable-offset fidelity** across ~15 interfaces — the main risk, per
  the Acrobat crash. Derive strictly from `MsHTML.h` `*Vtbl` structs;
  size-assert every interface; smoke-test early.
- **Change-sink correctness** — dynamic DOM updates are where MSHTML
  bugs hide; needs a dynamic test page.
- **fillVBuf feature completeness** — tables, list indices, preformatted
  text, atomic nodes: each needs a faithful port + a matching test case.

## Effort

Roughly 2–3× the Acrobat port: more interfaces (though each shallow), a
larger `fillVBuf`, a second helper module, and the COM change sink.
Multi-session. The payoff — the highest-traffic legacy backend on Rust —
plus after this only webKit + lotusNotesRichText (both niche) remain
before C++ `storage.cpp` can be considered for retirement.
