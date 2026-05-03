# Design: Rust port of `getTextFromIAccessible`

**Status:** Approved (2026-05-03)

## Goal

Port `getTextFromIAccessible` (the recursive IA2 text-extraction function in `nvdaHelper/remote/textFromIAccessible.cpp`) to Rust on x86_64, leaving the public C++ signature unchanged. Non-x86_64 keeps the verbatim C++ implementation under `#ifdef _M_X64`.

## Non-goals

* Porting `getAccessibleChildren` (thin Win32 wrapper, no logic — will dissolve when its 7 callers are ported).
* Porting `HyperlinkGetter`/`HtHyperlinkGetter`/`Ht2HyperlinkGetter` (polymorphic iterator over IA2 hypertext interface differences, sole caller is `gecko_ia2.cpp` — will dissolve when that caller is ported).
* Multi-arch cargo builds (still host-triple only).

Both helpers stay verbatim in C++ for this PR.

## Architecture

Mirror PR 1's pattern: Rust port lives in a new module in the existing `nvda_ia2` crate, exposed via an `extern "C"` shim. The C++ `getTextFromIAccessible` body becomes a thin wrapper on x86_64 that calls into the Rust shim. On other arches the original C++ body is preserved verbatim.

### Interface bindings

PR 1 declared the vtable layouts for `IAccessibleText`, `IAccessibleHypertext`, `IAccessibleHypertext2`, and `IAccessibleHyperlink` but only added Rust method wrappers for `IAccessible2::get_attributes`. PR 2 adds the wrappers needed for text extraction:

| Interface | Method | Notes |
| --- | --- | --- |
| `IAccessibleText` | `get_text(start, end) -> Result<BSTR>` | BSTR ownership transferred to Rust; freed on drop |
| `IAccessibleText` | `get_newText() -> Result<IA2TextSegment>` | `IA2TextSegment.text` is a server-allocated BSTR; wrapper takes ownership and drops on `IA2TextSegment` drop |
| `IAccessibleHypertext` | `get_hyperlink(index) -> Result<IAccessibleHyperlink>` | Standard COM interface ownership |
| `IAccessibleHypertext` | `get_hyperlinkIndex(char_index) -> Result<i32>` | Out-param, no ownership |

`IAccessible::get_accName`/`get_accDescription`/`get_accChildCount` and Win32 `AccessibleChildren` are taken from the `windows` crate directly — no hand-rolled bindings needed.

All wrappers carry the same `# Safety` documentation pattern as the existing `IAccessible2::get_attributes`, and free any BSTR a misbehaving server may have written before returning failure.

### FFI shape

```c
typedef void (*AppendCharsCallback)(
    void* ctx,
    const wchar_t* ptr,
    size_t len);

bool nvda_ia2_get_text_from_iaccessible(
    void* pacc2,
    bool use_new_text,
    bool recurse,
    bool include_top_level_text,
    void* ctx,
    AppendCharsCallback append_cb);
```

Same callback-bridge pattern as PR 1's `AttribCallback`. Rust accumulates characters into an internal `Vec<u16>` while recursing, then calls `append_cb(ctx, ptr, len)` once at the end. This is semantically equivalent to the C++ implementation's per-leaf `textBuf.append(...)` calls — both produce the same final string in the same order. Returns the `gotText` boolean.

The C++ wrapper:

```cpp
namespace {
    void append_chars(void* ctx, const wchar_t* ptr, size_t len) {
        static_cast<std::wstring*>(ctx)->append(ptr, len);
    }
}

bool getTextFromIAccessible(
    std::wstring& textBuf, IAccessible2* pacc2,
    bool useNewText, bool recurse, bool includeTopLevelText
) {
    return nvda_ia2_get_text_from_iaccessible(
        pacc2, useNewText, recurse, includeTopLevelText,
        &textBuf, append_chars);
}
```

### Pure-logic extraction

The only piece of `getTextFromIAccessible` that has logic separable from COM is `isEmpty`. Pulled out as:

```rust
fn is_empty_text(chars: &[u16]) -> bool {
    chars.iter().all(|&c| c == OBJ_REPLACEMENT_CHAR || is_whitespace_w(c))
}
```

Unit-tested directly with various inputs (empty, all spaces, all OBJ_REPLACEMENT_CHAR, mixed, with content). `is_whitespace_w` mirrors C runtime `iswspace` for the BMP characters that matter here (NVDA only sees BMP from BSTRs).

Everything else in `getTextFromIAccessible` is COM call orchestration; smoke test in Firefox is the integration gate.

## Data flow

```
C++ getTextFromIAccessible(textBuf, pacc2, ...)
  └─→ extern "C" nvda_ia2_get_text_from_iaccessible(pacc2, flags, &textBuf, append_chars)
        └─→ Rust get_text_from_iaccessible(pacc2: &IAccessible2, flags, &mut Vec<u16>)
              ├─ QI to IAccessibleText (Option)
              ├─ if !text && recurse && !use_new_text:
              │    AccessibleChildren -> for each VARIANT IDispatch child:
              │      QI to IAccessible2 → fetch attributes → check live → recurse
              ├─ if text:
              │    get_newText OR get_text(0, IA2_TEXT_OFFSET_LENGTH)
              │    QI to IAccessibleHypertext if recurse
              │    for each char in BSTR:
              │      if OBJ_REPLACEMENT_CHAR && hypertext:
              │        get_hyperlinkIndex → get_hyperlink → QI to IAccessible2
              │          → fetch attributes → check live → recurse
              │      else if include_top_level_text: push char
              ├─ if !got_text && !use_new_text:
              │    appendNameDescription via IAccessible.get_accName/get_accDescription
              └─ return got_text
        └─→ append_cb(ctx, vec.as_ptr(), vec.len())  // single call at end
  └─ (textBuf now contains the appended text)
```

## Error handling

* COM call failures (HRESULTs other than `S_OK`): treated as "no text" at that node, mirroring the C++ which checks specific success values and silently moves on otherwise.
* QI failures: `Option::None`, processing continues.
* BSTR ownership errors (server returns failure but writes a BSTR anyway): Rust wrapper takes ownership and drops, matching CComBSTR behavior.
* No panics on malformed input; behavior matches C++.

## Testing

Unit tests in Rust:

* `is_empty_text` with all relevant input shapes (empty slice, all spaces, all object-replacement chars, mixed whitespace + objrepl, content present).

Integration:

* Firefox smoke test (same gate as PR 1): browse a page with headings, links, embedded objects; confirm NVDA announces text correctly. Watch NVDA log for panics.

No COM-mock infrastructure (rejected as a tarpit during brainstorming — interface surfaces too wide for the value gained, and most risk is in COM orchestration which mocks won't faithfully exercise).

## File structure

**Modify:**

| File | Change |
| --- | --- |
| `rust/nvda_ia2/src/interfaces.rs` | Add 4 Rust method wrappers (`IAccessibleText::get_text`, `IAccessibleText::get_newText`, `IAccessibleHypertext::get_hyperlink`, `IAccessibleHypertext::get_hyperlinkIndex`). Vtable slot declarations from PR 1 are unchanged. |
| `rust/nvda_ia2/src/lib.rs` | Add `pub mod text;` |
| `nvdaHelper/remote/textFromIAccessible.cpp` | Replace function body with `#ifdef _M_X64` Rust shim delegation; preserve verbatim C++ in `#else` branch. |

**Create:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/src/text.rs` | `get_text_from_iaccessible` Rust port, `is_empty_text` pure helper with unit tests, `extern "C"` shim. |

## Commit plan

Each commit is self-contained and reviewable in isolation. If a future PR carve-up is wanted, the natural split point is after commit 3 (pure additions, no C++ touched) vs commits 4-5 (the actual C++ delegation).

1. Add `IAccessibleText::get_text` and `get_newText` Rust method wrappers.
2. Add `IAccessibleHypertext::get_hyperlink` and `get_hyperlinkIndex` Rust method wrappers.
3. Add `is_empty_text` pure helper + unit tests.
4. Add `get_text_from_iaccessible` impl and `extern "C"` shim.
5. Wire `nvdaHelper/remote/textFromIAccessible.cpp` to delegate on x86_64.

## Open questions

None. Design approved verbally before this doc was written.
