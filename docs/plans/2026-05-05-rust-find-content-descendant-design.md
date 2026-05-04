# Design: Rust port of `findContentDescendant`

**Status:** Approved (2026-05-05)

## Goal

Port `findContentDescendant` (`nvdaHelper/remote/IA2Support.cpp:229-312`) -- a recursive IA2 hypertext walk that locates a content descendant for caret / selection / first / last navigation -- to Rust on x86_64. The public C++ signature stays unchanged. Non-x86_64 keeps the verbatim C++ implementation under `#ifdef _M_X64`.

## Non-goals

* Porting the surrounding RPC infrastructure: `nvdaInProcUtils_IA2Text_findContentDescendant`, `getIA2(hwnd, parentID)`, `execInThread`, the WinEvent hooks (`IA2Support_winEventProcHook`), the COM proxy registration (`installIA2Support`/`uninstallIA2Support`), and the `IA2Support_inProcess_initialize`/`_terminate` plumbing. All of that stays C++ -- it's RPC / thread-marshaling / Win32 hook glue, not IA2 logic.
* Touching `nvdaInProcUtils_getTextFromIAccessible` (the sibling RPC entry that calls the already-ported `getTextFromIAccessible`).
* Multi-arch cargo builds.

## Architecture

Mirror PR 2 / PR 3 pattern. The Rust port lives in a new module in the `nvda_ia2` crate, exposed via an `extern "C"` shim. The C++ `findContentDescendant` body becomes a thin wrapper on x86_64 that calls into the Rust shim. The four `FINDCONTENTDESCENDANT_*` constants stay defined in the C++ file; Rust uses a parallel enum decoded from a `u32` tag.

### Interface bindings

PR 1 declared the `IAccessibleText` vtable layout and PR 2 added wrappers for `get_text` and `get_newText`. This PR adds the four wrappers `findContentDescendant` needs:

| Interface | Method | Notes |
| --- | --- | --- |
| `IAccessibleText` | `get_caretOffset(&self) -> Result<i32>` | Used for the CARET branch |
| `IAccessibleText` | `get_nCharacters(&self) -> Result<i32>` | Used for the LAST branch |
| `IAccessibleText` | `get_nSelections(&self) -> Result<i32>` | Used for SELECTIONSTART / SELECTIONEND branches |
| `IAccessibleText` | `get_selection(&self, index: i32) -> Result<(i32, i32)>` | Returns `(startOffset, endOffset)` |

`IAccessibleHypertext::get_hyperlinkIndex` and `get_hyperlink` are already wrapped (PR 2). `IAccessible::get_accChild` and `IAccessible::get_accChildCount` come from windows-rs directly. `IAccessible2::get_uniqueID` was added in PR 3.

### FFI shape

```c
bool nvda_ia2_find_content_descendant(
    void* pacc2,
    unsigned int what,
    int* descendant_id,
    int* descendant_offset);
```

`what` is a small enum tag (0..=4) the C++ side passes through. Rust decodes; an out-of-range tag yields `false` without writing the out-params.

The function returns the same `bool` the C++ does. On `true`, both out-params have been written. On `false`, neither has been (the C++ original is similarly contract-bound to the caller's reads, since callers only read on `true`).

The C++ wrapper:

```cpp
bool findContentDescendant(IAccessible2* pacc2, long what, long* descendantID, long* descendantOffset) {
    int id = 0, off = 0;
    bool ok = nvda_ia2_find_content_descendant(
        pacc2, static_cast<unsigned int>(what), &id, &off);
    if (ok) {
        *descendantID = id;
        *descendantOffset = off;
    }
    return ok;
}
```

(C++ uses `long` -- 32-bit on Windows/MSVC -- so the `i32` <-> `long` round-trip is bit-equivalent.)

### Pure-logic extraction

Marginal. The only piece of `findContentDescendant` that has logic separable from COM is the selection-offset selector:

```rust
fn pick_selection_offset(what: FindWhat, start: i32, end: i32) -> i32 {
    match what {
        FindWhat::SelectionStart => start,
        _ => end - 1, // treated as SelectionEnd by the caller
    }
}
```

That's a one-liner. Not worth a dedicated helper -- just inline it.

The interesting integration testing is end-to-end through Firefox's IA2 caret movement; no COM-mock unit tests (rejected as a tarpit during PR 2 brainstorming, principle still applies).

## Data flow

```
C++ findContentDescendant(pacc2, what, &id, &off)
  └─→ extern "C" nvda_ia2_find_content_descendant(pacc2, what, &id, &off)
        └─→ Rust find_content_descendant(pacc2: &IAccessible2, what: FindWhat) -> Option<(i32, i32)>
              ├─ QI to IAccessibleText
              ├─ if has IAccessibleText:
              │    compute offset via what:
              │      First -> 0
              │      Caret -> get_caretOffset
              │      Last -> max(0, get_nCharacters - 1)
              │      SelectionStart/End -> get_nSelections; if 0 -> -1; else get_selection(0)
              │    if offset == -1: return None
              │    QI IAccessibleHypertext, get_hyperlinkIndex(offset);
              │    if hi >= 0:
              │      get_hyperlink(hi) -> IAccessibleHyperlink -> QI IAccessible2
              │      recurse with same `what`; if !found and what == Caret, recurse with First
              │    fallback: pacc2.get_uniqueID(), offset -> Some((id, offset))
              └─ else:
                   for i in 1..=accChildCount (reversed for Last/SelectionEnd):
                       get_accChild(i) -> IDispatch -> QI IAccessible2 -> recurse
                       break on first found
```

## Error handling

* COM failures: silent return `None` at that node, mirroring C++ which lets failed `QueryInterface` / `get_hyperlink` calls produce null pointers and skips the branch.
* Out-of-range `what`: shim returns `false`.
* No panics on malformed input.

## Testing

* No new unit tests (the offset selector is the only candidate and it's a one-liner).
* Integration: smoke-test in Firefox by exercising caret navigation in a structured page (move into a heading or link, observe NVDA's caret-following behavior). The function is invoked via the RPC entry on every focus / caret event NVDA observes against a Gecko document.

## File structure

**Modify:**

| File | Change |
| --- | --- |
| `rust/nvda_ia2/src/interfaces.rs` | Promote 4 vtable slots (`get_caretOffset`, `get_nSelections`, `get_selection`, `get_nCharacters`) from `usize` to typed function pointers, add Rust wrappers |
| `rust/nvda_ia2/src/lib.rs` | Add `pub mod find_descendant;` |
| `nvdaHelper/remote/IA2Support.cpp` | Replace the body of `findContentDescendant` with a `#ifdef _M_X64` Rust-shim wrapper; preserve verbatim C++ in `#else` |

**Create:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/src/find_descendant.rs` | `find_content_descendant` Rust port + `extern "C"` shim |

## Commit plan

1. Add `IAccessibleText::get_caretOffset`, `get_nCharacters`, `get_nSelections`, `get_selection` Rust wrappers.
2. Add `find_descendant` module with `find_content_descendant` impl + `extern "C"` shim.
3. Wire `nvdaHelper/remote/IA2Support.cpp` to delegate `findContentDescendant` on x86_64.

PR carve-up: commits 1+2 (no C++ touched, additive) vs commit 3 (the actual delegation). Or 1 alone (just IAccessibleText wrappers, useful for future ports too) vs 2+3.

## Open questions

None. The design space is constrained -- the function signature, wrapper shapes, and FFI pattern are all forced by the existing patterns in PR 1/2/3 plus the C++ caller's contract.
