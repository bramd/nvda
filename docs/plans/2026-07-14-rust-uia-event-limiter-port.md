# Porting the UIA event rate-limiter to Rust

**Status:** assessment + plan (2026-07-14). Feasibility confirmed by a full
survey (C++ core read directly + an agent map of the Python integration +
windows-rs capability checks). Not started — awaiting go-ahead.

## What it is

`nvdaHelper/local/UIAEventLimiter/` — a COM object (`RateLimitedEventHandler`)
that NVDA registers with UI Automation as the handler for every event type.
On the modern UIA path (`config["UIA"]["enhancedEventProcessing"]`, **default
enabled**) it sits between UIA core and NVDA's Python UIA handler:

1. **Receive** — implements 5 `IUIAutomation*EventHandler` interfaces
   (+ `IUnknown`); UIA core calls `Handle*Event` on its threads.
2. **Coalesce** — queues each event, de-duplicating by a coalescing key
   (element RuntimeId + event/property/notification discriminators),
   last-write-wins, keeping the newest at the back of an insertion-ordered
   queue. `std::list` + `std::map<key,{iter,count}>` (the `std::variant` of 5
   record types is a textbook sum type).
3. **Flush** — a single `std::jthread` + `condition_variable` wakes on the
   first insert, swaps the queue out under the lock, and emits each event
   **outside the lock** to the wrapped "existing handler".

**The wrapped handler is NVDA's Python `UIAHandler` COMObject** (comtypes),
passed to `create()` as `self.QueryInterface(IUnknown)`. So the flush thread's
`emit` makes real COM `Handle*Event` calls **back into Python**. This is the
whole point: it keeps UIA core from ever blocking on Python.

## Why it's a strong Rust target

* **Modern & hot** — `enhancedEventProcessing` defaults to *enabled*; every
  focus / property / notification / text-position change in every UIA app
  flows through this. (Unlike the GDI/OCM code, which UIA has superseded.)
* **The best Rust-fit shape yet** — a concurrent de-duplicating queue. Rust
  turns the error-prone parts into compile-time guarantees: the `std::variant`
  * `concept`/`supports_alternative` template machinery → a plain `enum`; the
  manual `QueryInterface`/`AddRef`/`Release` → windows-rs `#[implement]`; the
  hand-managed `jthread`/mutex/condvar → `Mutex`+`Condvar` with `Send`/`Sync`
  checked by the compiler (data races and dropped/duplicated events are
  exactly the bugs that matter here).
* **Genuinely unit-testable** — the coalescing-key + ordered-dedup core is
  pure logic; there are **no tests today**, so the port establishes coverage.

## Capability checks (done)

* windows-rs 0.58 exposes all 5 `IUIAutomation*EventHandler_Impl` traits
  (`Win32/UI/Accessibility/impl.rs`) → **`#[implement]` works** once the
  `windows` crate's `implement` feature is enabled. `GetRuntimeId`,
  `Add*EventHandler`, `NotificationKind`/`NotificationProcessing` are all
  present.
* The C ABI Python depends on (`NVDAHelper/localLib.py:491-504`) is two flat
  exports from **nvdaHelperLocal.dll** (`.def:71-72`):
  `rateLimitedUIAEventHandler_create(IUnknown* existing, void** out)` and
  `rateLimitedUIAEventHandler_terminate(void* handle)`. The port must preserve
  these **and** the contract that the returned handle is a real COM object
  implementing IUnknown + the 5 interfaces (UIA registers and refcounts it).

## Design decisions

**D1 — Full Rust via `#[implement]`, not a C++ shell.** One
`#[implement(IUIAutomationEventHandler, …4 more…)]` struct; windows-rs
generates IUnknown/QueryInterface/refcount. The two C exports become Rust
`#[no_mangle] extern "C"`. Deletes all the C++ (`api.cpp`,
`rateLimitedEventHandler.{h,cpp}`, `eventRecord.h`, `utils.{h,cpp}`) and keeps
the `.def` exports + ctypes unchanged, so Python needs no change.

**D2 — Cross-thread COM refs (the crux).** Records hold
`IUIAutomationElement` / `IUIAutomationTextRange` / `VARIANT` / `BSTR`, moved
callback-thread → queue → flush-thread. windows-rs COM types are `!Send`. The
C++ moves them across threads with raw pointers, relying on UIA objects being
*agile*. Two options:

* **(a) documented `unsafe impl Send` newtype** asserting that agility —
  reproduces today's shipping behavior exactly, zero overhead. **Recommended**
  for a faithful port.
* **(b) `AgileReference<T>`** (RoGetAgileReference; `runtimeobject.lib` is
  already linked) — correct-by-construction cross-apartment marshaling, slight
  per-object cost, subtly different behavior.
This is the one decision worth an explicit review call.

**D3 — Ordered-dedup queue: `indexmap::IndexMap<Vec<i32>, EventRecord>`** —
insertion order + O(1) key lookup; coalesce = `shift_remove` + `insert` to
move-newest-to-back. One small, well-vendored dependency. (The C++ `count`
field is tracked but never emitted — dropping it is behaviour-preserving.)

**D4 — Threading.** Struct holds an `Arc<Shared>` (the `Mutex<IndexMap>` +
`Condvar` + stop flag + the existing-handler ref) and a `JoinHandle`; the
flush thread holds a clone of the `Arc`. **The flush thread must NOT hold a
strong COM ref to the handler object** — that would be a refcount cycle that
never frees. `terminate()` sets the stop flag, notifies, and **joins**
(blocking, as today) before UIA/Python drop their COM refs and windows-rs
frees the object. This ordering is the careful part.

**D5 — Coalescing key.** Port `getRuntimeIDFromElement` /
`SafeArrayToVector`: `IUIAutomationElement::GetRuntimeId()` → read the
`SAFEARRAY` of `i32` → `Vec<i32>`, then push the per-event discriminators.

## Phased plan (build + commit each)

* **Phase 0 — build spike.** New crate `nvda_uia_events` (staticlib+rlib;
  `windows` features `implement` + `Win32_UI_Accessibility` + variant/SAFEARRAY
  bits). Wire the **first Rust staticlib into nvdaHelperLocal.dll** — replicate
  the proven `remote/sconscript` cargo integration for `local/sconscript`.
  Prove an empty `#[implement]`-of-5-interfaces object compiles and the DLL
  links. De-risks the two novel bits (local-DLL Rust + `#[implement]`).
* **Phase 1 — pure dedup core + unit tests.** The `EventRecord` enum,
  `generate_coalescing_key`, and the ordered-dedup queue (last-write-wins,
  move-to-back). Full unit tests with synthetic keys — no COM, no threads.
* **Phase 2 — COM `#[implement]` + threading + C ABI.** The 5 handler vtables
  (queue on receipt), the D2 Send-wrapped records, the flush thread (D4),
  emit-back-into-the-existing-handler, `GetRuntimeId` (D5), and the two
  `#[no_mangle]` `create`/`terminate` exports matching the current ABI.
* **Phase 3 — flip + delete C++.** Point `local/sconscript` at the Rust crate;
  keep the two `.def` exports; delete the five C++ files. Build
  nvdaHelperLocal.dll. **Manual smoke test** in a UIA app (modern Notepad /
  Edge / Office) with `enhancedEventProcessing` on (default): confirm events
  still reach NVDA and nothing floods or stalls.

## Risk summary

* ~~`#[implement]` for UIA handlers~~ — **confirmed supported** (enable the
  `implement` feature).
* **Cross-thread COM (`!Send`)** — D2; faithful port uses a documented `Send`
  wrapper matching the C++ agility assumption. Main review point.
* **Refcount-cycle / terminate-join ordering** — D4; flush thread holds an
  `Arc`, never a strong COM ref; `terminate()` joins before final release.
* **First Rust in nvdaHelperLocal.dll** — new build integration; Phase 0 spike.
* **No tests today** — Phase 1 builds the pure-logic coverage; the COM/thread
  path needs the manual NVDA smoke test. (Also confirm the local DLL's target
  arch(es) so both are rebuilt.)

Net effect: ~600 lines of template-heavy, manually-thread-safe C++ replaced by
a smaller Rust crate with a unit-tested dedup core, an idiomatic sum type, and
compiler-checked thread safety — on NVDA's default modern event path.
