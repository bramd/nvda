# Design: Rust port of `HyperlinkGetter`

**Status:** Draft (2026-05-05)

## Goal

Port the `HyperlinkGetter` family (`HtHyperlinkGetter`, `Ht2HyperlinkGetter`, the abstract base, and the `makeHyperlinkGetter` factory) from `nvdaHelper/common/ia2utils.cpp` to Rust on x86_64. The sole caller (`nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:1055`) keeps using the existing public C++ API (`makeHyperlinkGetter` + `HyperlinkGetter::next`) -- the change is invisible at the call site. Non-x86_64 keeps the verbatim C++ implementation under `#ifdef _M_X64`.

This is the first FFI pattern in this porting effort that exposes a stateful Rust object to C++ via an opaque pointer with explicit `make` / `next` / `free` lifecycle.

## Non-goals

* Porting `getAccessibleChildren` (still deferred).
* Touching `gecko_ia2.cpp` -- the caller stays as-is.
* Multi-arch cargo builds.

## Architecture

The Rust port lives in a new `hyperlink_getter` module in the `nvda_ia2` crate. The C++ side keeps the public `HyperlinkGetter` abstract base with `next() -> CComPtr<IAccessibleHyperlink>`, but on x86_64 the only implementation is a thin RAII wrapper holding a `*mut c_void` Rust handle. The pre-existing `HtHyperlinkGetter` / `Ht2HyperlinkGetter` subclasses move from the public header into the `#else` branch of `ia2utils.cpp`, so they are no longer part of the public API.

### Header reshape

PR-3 / PR-4 didn't touch headers. This PR does. Specifically:

* `ia2utils.h`: remove the public `HtHyperlinkGetter` and `Ht2HyperlinkGetter` class declarations (they were never instantiated outside `ia2utils.cpp`). Promote `HyperlinkGetter::next()` from "virtual with default impl that calls `get()`" to pure virtual. The protected `index` field and `get()` virtual move into the non-x86_64-only subclasses (private to the .cpp file).

The header becomes:

```cpp
class HyperlinkGetter {
    public:
    virtual ~HyperlinkGetter() {}
    virtual CComPtr<IAccessibleHyperlink> next() = 0;
};

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc);
```

This is a strictly smaller public API. We verified by `grep` that the only external consumer of `HyperlinkGetter` is `gecko_ia2.cpp`, and it uses only `makeHyperlinkGetter` and `next()`.

### FFI shape

Three new `extern "C"` functions:

```c
/* Allocates and returns an opaque handle, or NULL if the IAccessible2
 * supports neither IAccessibleHypertext2 nor IAccessibleHypertext.
 */
void* nvda_ia2_make_hyperlink_getter(void* pacc2);

/* Returns the next hyperlink (AddRef'd; caller must Release), or NULL
 * if iteration is exhausted. Increments the internal index.
 */
void* nvda_ia2_hyperlink_getter_next(void* handle);

/* Drops the Rust state, including any cached IAccessibleHyperlink
 * references the Ht2 variant collected via get_hyperlinks.
 */
void nvda_ia2_hyperlink_getter_free(void* handle);
```

Same null-or-handle convention as PR 1's `IA2AttribsToMap` / PR 2's `getTextFromIAccessible` shims, just with persistent state between calls.

### Rust internals

```rust
pub enum HyperlinkGetter {
    /// IAccessibleHypertext path: fetch one hyperlink at a time via
    /// get_hyperlink(index). Mirrors HtHyperlinkGetter.
    Ht {
        hypertext: IAccessibleHypertext,
        index: u32,
    },
    /// IAccessibleHypertext2 path: fetch all hyperlinks up front via
    /// get_hyperlinks (server-allocated CoTaskMem array of IUnknown*),
    /// then index into the cached Vec on each next(). Mirrors
    /// Ht2HyperlinkGetter.
    Ht2 {
        // Lazily fetched on first next() call.
        links: Option<Vec<Option<IAccessibleHyperlink>>>,
        hypertext: IAccessibleHypertext2,
        index: u32,
    },
}
```

The `next()` method returns `Option<IAccessibleHyperlink>` (cloned/AddRef'd). The opaque shim wraps `Box<HyperlinkGetter>`.

**One incidental fix vs. the C++ original:** `Ht2HyperlinkGetter::~Ht2HyperlinkGetter` calls `CoTaskMemFree(rawLinks)` but does NOT `Release` the `IUnknown*` entries that the caller never iterated to. That's a leak the existing C++ has carried since the helper was written -- per the IDL contract for `get_hyperlinks`, every entry in the array is AddRef'd, and the client owns those references. Rust's `Drop` for `Vec<Option<IAccessibleHyperlink>>` releases them all naturally, so the Rust port fixes the leak as a side-effect. We'll mention this in the commit message.

### IAccessibleHypertext2 binding

PR 1 declared the `IAccessibleHypertext2` vtable layout but stubbed `get_hyperlinks` as `usize`. This PR promotes that slot to a typed function pointer and adds the wrapper:

```rust
pub unsafe fn get_hyperlinks(&self) -> Result<(*mut *mut c_void, i32)>
```

The output is the raw `IUnknown**` array + length pair. The Rust wrapper does NOT take ownership of the elements -- the caller (the `Ht2` constructor) walks the array, QIs each element to `IAccessibleHyperlink`, and stores them in a `Vec<Option<IAccessibleHyperlink>>`. After collection, the array itself is freed via `CoTaskMemFree`.

### C++ on x86_64

`ia2utils.cpp` adds a private subclass:

```cpp
namespace {
    class RustHyperlinkGetter : public HyperlinkGetter {
        void* handle;
    public:
        explicit RustHyperlinkGetter(void* h) : handle(h) {}
        ~RustHyperlinkGetter() override {
            if (handle) nvda_ia2_hyperlink_getter_free(handle);
        }
        // No copy, no assign.
        RustHyperlinkGetter(const RustHyperlinkGetter&) = delete;
        RustHyperlinkGetter& operator=(const RustHyperlinkGetter&) = delete;

        CComPtr<IAccessibleHyperlink> next() override {
            CComPtr<IAccessibleHyperlink> link;
            if (!handle) return link;
            auto* raw = static_cast<IAccessibleHyperlink*>(
                nvda_ia2_hyperlink_getter_next(handle));
            link.Attach(raw);  // raw is already AddRef'd or null
            return link;
        }
    };
}

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc) {
    void* handle = nvda_ia2_make_hyperlink_getter(acc);
    if (!handle) return nullptr;
    return std::make_unique<RustHyperlinkGetter>(handle);
}
```

### C++ on non-x86_64

The existing `Ht/Ht2HyperlinkGetter` definitions move from the header into a `#else` block in `ia2utils.cpp`. They become file-local. The polymorphic implementation is preserved verbatim (including the `index` field, the `next()` default, and the small leak in `Ht2HyperlinkGetter::~Ht2HyperlinkGetter`).

## Data flow

```
gecko_ia2.cpp: auto getter = makeHyperlinkGetter(pacc); ... getter->next()
  └─ x86_64: RustHyperlinkGetter wraps Rust handle
        nvda_ia2_make_hyperlink_getter(pacc2):
          QI to IAccessibleHypertext2 -> Some(Ht2 { hypertext, index: 0, links: None })
          else QI to IAccessibleHypertext -> Some(Ht { hypertext, index: 0 })
          else -> NULL
        nvda_ia2_hyperlink_getter_next(handle):
          Ht: get_hyperlink(index); index += 1; return AddRef'd link or null
          Ht2 (first call): get_hyperlinks() -> Vec<Option<IAccessibleHyperlink>>
                            (each entry QI'd from IUnknown to IAccessibleHyperlink)
                            CoTaskMemFree the IUnknown** array
          Ht2 (subsequent): take links[index]; index += 1; return AddRef'd or null
        nvda_ia2_hyperlink_getter_free(handle):
          drop(Box::from_raw(handle)); Release every cached interface

  └─ non-x86_64: HtHyperlinkGetter / Ht2HyperlinkGetter (unchanged)
```

## Error handling

* `nvda_ia2_make_hyperlink_getter` returns `NULL` on null input or when neither IAccessibleHypertext nor IAccessibleHypertext2 is supported. C++ `makeHyperlinkGetter` translates `NULL` to `nullptr` `unique_ptr`.
* `nvda_ia2_hyperlink_getter_next` returns `NULL` on null handle, exhausted iterator, or any COM failure. The C++ `Attach(null)` is a no-op.
* `nvda_ia2_hyperlink_getter_free(NULL)` is a no-op.
* No panics across the FFI boundary.

## Testing

No new unit tests -- the Rust impl is mostly COM call orchestration, and we already established (PR 2 / PR 3) that COM mocks aren't worth the maintenance cost.

Integration: smoke-test in Firefox by browsing a page with multiple links inside paragraphs. NVDA's browse-mode should still announce link text correctly. The Rust impl fixes a small Ht2 leak (uniterated entries get Released on drop, which the C++ original neglected) -- not user-observable, but slightly better.

## File structure

**Modify:**

| File | Change |
| --- | --- |
| `rust/nvda_ia2/src/interfaces.rs` | Promote `IAccessibleHypertext2::get_hyperlinks` from `usize` to typed function pointer; add Rust wrapper |
| `rust/nvda_ia2/src/lib.rs` | Add `pub mod hyperlink_getter;` |
| `nvdaHelper/common/ia2utils.h` | Reduce public API to `HyperlinkGetter` (pure-virtual `next()`) and `makeHyperlinkGetter` only; remove Ht/Ht2 class declarations |
| `nvdaHelper/common/ia2utils.cpp` | On x86_64, implement `RustHyperlinkGetter` subclass + `makeHyperlinkGetter` shim; on non-x86_64, move Ht/Ht2 definitions into `#else` branch (verbatim) |

**Create:**

| File | Responsibility |
| --- | --- |
| `rust/nvda_ia2/src/hyperlink_getter.rs` | `HyperlinkGetter` enum, `next()` method, three `extern "C"` shims |

## Commit plan

1. Add `IAccessibleHypertext2::get_hyperlinks` Rust wrapper in `interfaces.rs`.
2. Add `hyperlink_getter` Rust module + three `extern "C"` shims (no C++ changes; pure additive).
3. Refactor `ia2utils.h` to reduce the public API; move Ht/Ht2 class definitions to `ia2utils.cpp`'s body. No behavior change on either arch yet.
4. Wire `ia2utils.cpp` x86_64 branch to use the Rust shims via `RustHyperlinkGetter`; keep `#else` verbatim.

PR carve-up suggestion:

* PR A = commits 1 + 2 (Rust additions, no C++ changes). Independent.
* PR B = commits 3 + 4 (header reshape + delegation). Depends on PR A landing first.

## Open questions

None. Design space is constrained: the existing C++ class hierarchy guides the FFI surface, and the opaque-pointer iterator pattern is the standard idiom for stateful Rust-from-C++ in this codebase shape.
