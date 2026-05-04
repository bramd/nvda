# Design: Rust port of `ia2LiveRegions`

**Status:** Draft (2026-05-04)

## Goal

Port `nvdaHelper/remote/ia2LiveRegions.cpp` to Rust on x86_64, leaving the public C++ entry points (`ia2LiveRegions_inProcess_initialize`/`_terminate`) and the `WINEVENTPROC` callback signature unchanged. Non-x86_64 keeps the verbatim C++ implementation under `#ifdef _M_X64`.

## Non-goals

* Porting the `WINEVENTPROC` callback registration (`registerWinEventHook` in `nvdaHelperRemote.cpp`). The hook callback can't carry the `WINEVENTPROC` ABI from a `staticlib` cleanly without per-instance trampolines we have no use for; the C++ shim is the natural landing spot for the Win32-only setup (event-type filter, `AccessibleObjectFromEvent`, accState fetch, QI to `IAccessible2`).
* Touching `nvdaControllerInternal_reportLiveRegion`. Rust calls back into C++ through the same callback-bridge pattern PR 1 (`AttribCallback`) and PR 2 (`AppendCharsCallback`) use.
* Multi-arch cargo builds.

## Architecture

Mirror PR 2's pattern. The Rust port lives in a new `live_regions` module in the existing `nvda_ia2` crate, exposed via an `extern "C"` shim. The C++ `winEventProcHook` becomes a thin wrapper on x86_64 that:

1. Does the Win32-only setup (filter by event type and window visibility, `AccessibleObjectFromEvent`, get `accState`, bail on `STATE_SYSTEM_INVISIBLE`, QI to `IAccessible2`).
2. Calls the Rust shim, which runs the entire IA2-attribute predicate chain, the `findAriaAtomic` walk, the background-tab check, the text retrieval, and reports back via callback.
3. Forwards the callback to `nvdaControllerInternal_reportLiveRegion`.

Non-x86_64 keeps the original C++ body verbatim under `#else`.

### Interface bindings

PR 1 declared the `IAccessible2` vtable layout but only added a Rust wrapper for `get_attributes`. PR 2 added the `IAccessibleText`/`IAccessibleHypertext` wrappers needed for text extraction. This PR adds two more `IAccessible2` wrappers:

| Interface | Method | Notes |
| --- | --- | --- |
| `IAccessible2` | `get_states(&self) -> Result<i32>` | IA2 state bitmask, used for the `IA2_STATE_EDITABLE` filter |
| `IAccessible2` | `get_uniqueID(&self) -> Result<i32>` | Used for foreground-tab vs. event-tab comparison |

`IAccessible::accNavigate`, `IAccessible::get_accParent`, and `IServiceProvider::QueryService` are already on the `windows` crate's typed interfaces -- no hand-rolled bindings needed. `AccessibleObjectFromWindow` and the `STATE_SYSTEM_*` / `IA2_*` constants likewise come from the `windows` crate as-is.

### Pure-logic extraction

The IA2-attribute predicate chain is mostly pure logic over the `BTreeMap<String, String>` that `parse_attribs` already produces. Pulled out as plain Rust functions, easy to unit-test:

```rust
pub enum LivePoliteness { Polite, Assertive, Rude }
pub fn parse_live_politeness(map: &BTreeMap<String, String>) -> Option<LivePoliteness>;

pub struct Relevance { pub additions: bool, pub text: bool }
pub fn parse_container_relevant(map: &BTreeMap<String, String>) -> Relevance;

pub fn is_container_busy(map: &BTreeMap<String, String>) -> bool;
pub fn is_atomic(map: &BTreeMap<String, String>) -> bool;
pub fn is_container_atomic(map: &BTreeMap<String, String>) -> bool;
```

Plus a small enum for the WinEvent IDs the hook actually cares about (`EVENT_OBJECT_NAMECHANGE`, `EVENT_OBJECT_DESCRIPTIONCHANGE`, `EVENT_OBJECT_SHOW`, `IA2_EVENT_TEXT_UPDATED`, `IA2_EVENT_TEXT_INSERTED`).

The COM-orchestration parts (`find_aria_atomic`, `is_in_background_tab`, `ia2_unique_id_from_dispatch_variant`) are smaller; integration is gated by Firefox smoke test, no COM mocks (rejected as a tarpit during PR 2 brainstorming).

### FFI shape

```c
typedef void (*ReportLiveRegionCallback)(
    void* ctx,
    const wchar_t* text_ptr,     size_t text_len,
    const wchar_t* politeness_ptr, size_t politeness_len);

bool nvda_ia2_handle_live_region_event(
    void* pacc2,             /* borrowed IAccessible2* */
    void* hwnd,              /* HWND, opaque */
    unsigned int event_id,   /* DWORD WinEvent ID */
    int acc_state,           /* lVal from varState; 0 if VT was not VT_I4 */
    void* ctx,
    ReportLiveRegionCallback report_cb);
```

Rust collects the (text, politeness) pair and invokes `report_cb` once at the end if the event passes all filters. Returns `true` if the callback was invoked. The C++ caller doesn't need the boolean -- it's there for parity with PR 2 and to make the contract observable in tests.

`acc_state = 0` for "not `VT_I4`" is sound: every check is `state & FLAG`, which short-circuits on 0. The C++ already did the `STATE_SYSTEM_INVISIBLE` early-return before reaching this shim, so the only state bit Rust still cares about is `STATE_SYSTEM_OFFSCREEN`, which gates the background-tab check; state 0 simply skips that gate -- benign.

The C++ wrapper:

```cpp
namespace {
    void report_live(void* ctx, const wchar_t* text_ptr, size_t text_len,
                     const wchar_t* polite_ptr, size_t polite_len) {
        try {
            std::wstring text(text_ptr, text_len);
            std::wstring polite(polite_ptr, polite_len);
            nvdaControllerInternal_reportLiveRegion(text.c_str(), polite.c_str());
        } catch (const std::bad_alloc&) {
            // Suppressed to prevent UB from a C++ exception crossing the
            // extern "C" frame back into Rust.
        }
    }
}
```

## Data flow

```
C++ winEventProcHook(hookId, eventID, hwnd, objectID, childID, threadID, time)
  ├─ event-type filter (early return)
  ├─ window-visibility filter
  ├─ AccessibleObjectFromEvent → IAccessible
  ├─ get_accState → invisible filter
  ├─ QI IServiceProvider → IAccessible2 (early return on failure)
  └─→ extern "C" nvda_ia2_handle_live_region_event(pacc2, hwnd, event_id, acc_state, ctx, report_cb)
        └─→ Rust handle_live_region_event(...)
              ├─ fetchIA2Attributes (already in Rust from PR 1)
              ├─ container-live filter → LivePoliteness
              ├─ if STATE_SYSTEM_OFFSCREEN: is_in_background_tab → if true, bail
              ├─ get_states → IA2_STATE_EDITABLE filter
              ├─ container-busy filter
              ├─ container-relevant parse → Relevance
              ├─ EVENT_OBJECT_SHOW edge case (parent has IAccessibleText OR
              │   parent has its own valid container-live → ignore)
              ├─ allow_text filter for NAMECHANGE / DESCRIPTIONCHANGE
              ├─ find_aria_atomic walk → choose atomic ancestor or self
              ├─ getTextFromIAccessible (already in Rust from PR 2)
              └─→ if got_text && !text.empty: report_cb(ctx, &text, &politeness)
  └─ report_live → nvdaControllerInternal_reportLiveRegion
```

## Error handling

* COM failures (HRESULTs other than `S_OK`): silent early return at that node, mirroring the C++ which does the same.
* QI failures: `Option::None`, processing continues or bails as the C++ does.
* Malformed attribute strings: `parse_attribs` already handles this from PR 1; downstream predicates see whatever it produced.
* No panics on malformed input.

## Testing

Unit tests in Rust:

* `parse_live_politeness` for absent / `polite` / `assertive` / `rude` / `off` / unknown.
* `parse_container_relevant` for absent / `all` / `additions` / `text` / `additions text` / `text additions` / unrecognized.
* `is_container_busy`, `is_atomic`, `is_container_atomic` happy/sad paths.

Integration:

* Firefox smoke test: trigger `aria-live="polite"` and `aria-live="assertive"` updates and confirm NVDA announces both. Confirm `aria-live="off"` is silent. Confirm a `container-busy=true` ancestor suppresses. Confirm `aria-atomic=true` reads the whole region rather than just the changed text. Confirm a background tab does not announce.

## File structure

**Modify:**

| File | Change |
| --- | --- |
| `rust/nvda_ia2/src/interfaces.rs` | Add `IAccessible2::get_states` and `get_uniqueID` Rust method wrappers (vtable slots already declared in PR 1) |
| `rust/nvda_ia2/src/lib.rs` | Add `pub mod live_regions;` |
| `nvdaHelper/remote/ia2LiveRegions.cpp` | Replace the body of `winEventProcHook` (after the QI to `IAccessible2`) with `#ifdef _M_X64` Rust-shim delegation; preserve verbatim C++ in `#else` |

**Create:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/src/live_regions.rs` | Pure attribute predicates with unit tests, `find_aria_atomic` and `is_in_background_tab` Rust ports, the `handle_live_region_event` body, and the `extern "C"` shim |

## Commit plan

Each commit is self-contained and reviewable in isolation. Commit boundaries chosen so a future PR carve-up is straightforward.

1. Add `IAccessible2::get_states` and `get_uniqueID` Rust method wrappers.
2. Add `live_regions` module with the pure attribute predicates (`parse_live_politeness`, `parse_container_relevant`, `is_container_busy`, `is_atomic`, `is_container_atomic`) + unit tests.
3. Add `find_aria_atomic` recursive port (uses `IAccessible2::get_attributes`, `IAccessible::get_accParent`, IID cast).
4. Add `is_in_background_tab` port + helper `ia2_unique_id_from_dispatch_variant`.
5. Add `handle_live_region_event` glue and the `extern "C"` shim.
6. Wire `nvdaHelper/remote/ia2LiveRegions.cpp` to delegate on x86_64.

Natural future-PR carve-up:

* **PR A** = commits 1-2 (pure additions, no C++ touched, no behavior change).
* **PR B** = commits 3-4 (more Rust ports, still no C++ touched, no behavior change).
* **PR C** = commits 5-6 (shim + delegation; the actual behavior change).

PR A alone has no observable effect; folding it into PR B is also reasonable if "useful per PR" matters more than independence.

## Open questions

* The `EVENT_OBJECT_SHOW`-with-text-parent edge case (lines 197-205 of `ia2LiveRegions.cpp`) needs an `IAccessibleText` QI on the parent and an attribute fetch. Current plan: keep in Rust for testability; Rust already has the `IAccessibleText` vtable from PR 2.
* `accNavigate` returns a `VARIANT` whose discriminant is `VT_DISPATCH`. The PR 2 helper `variant_dispatch_ptr` (currently private to `text.rs`) extracts the `pdispVal`; we'll lift it into a shared module location when PR 4 needs it. Mechanical move, no behavior change -- could land as a no-op refactor commit before commit 4 above, or inline in commit 4.
