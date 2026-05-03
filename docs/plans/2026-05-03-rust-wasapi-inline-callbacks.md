# WASAPI inline callbacks — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the C++ behavior of firing audio "feed-done" callbacks inline during the WASAPI feed loop, instead of deferring them until `feed()` returns. Closes the perceived sluggishness in TTS index notifications (`synthIndexReached`) that the user reported.

**Architecture:** The inner Rust crate `nvda_wasapi` already takes `Box<dyn Fn(u32) + Send>` for its callback — that interface is fine and unchanged. The fix lives entirely in the PyO3 wrapper at `rust/nvda_python/src/wasapi.rs`: replace the queue-and-drain pattern (`pending_callbacks: Vec<u32>` + `fire_pending_callbacks(py)` after `feed()` returns) with an inline closure that uses `Python::attach(|py| ...)` to briefly re-acquire the GIL inside the feed loop and call the user's Python callback directly. This mirrors what C++ did via WINFUNCTYPE — the GIL was acquired implicitly per callback invocation.

**Tech Stack:** PyO3 0.28 (`Py<PyAny>::clone_ref`, `Python::attach`), no new dependencies, no version bumps.

---

## Why this matters

C++'s `WasapiPlayer::feed()` calls `maybeFireCallback()` after each buffer write, which directly invokes the registered Python callback (a WINFUNCTYPE that implicitly acquires the GIL). So when audio passes a feed-id's endpoint mid-loop, the corresponding callback fires immediately — even while `feed()` is still blocked waiting for the buffer to drain.

Rust's PyO3 wrapper currently:

1. Constructs the inner-crate callback as `Box::new(move |feed_id: u32| { pending_clone.lock().unwrap().push(feed_id); })`.
2. The inner `feed()` loop calls this closure, which only pushes to a `Vec`.
3. After `feed()` returns, the wrapper calls `fire_pending_callbacks(py)` which drains the Vec and invokes Python.

Net effect: callbacks queued during the feed loop wait until `feed()` returns. When `feed()` blocks ~100–200 ms waiting for the audio buffer to drain (the `padding > buffer_frames / 2` branch in `nvda_wasapi/src/player.rs`), all callbacks for already-played audio are stuck behind that wait.

For TTS where `synthIndexReached` notifications drive cursor highlighting, focus tracking, and "ready for next utterance" signaling, this delay is felt as sluggishness. The deferred-callback pattern was a defensible choice when implemented (avoids re-acquiring GIL inside the inner crate), but the perf cost of GIL re-acquire per callback (~1–5 μs) is negligible compared to the ~100 ms wait it currently sits behind.

---

## Scope

**In scope:**

* Replace the `pending_callbacks` Vec + `fire_pending_callbacks` drain in `nvda_python/src/wasapi.rs` with an inline GIL-reacquiring closure.
* Verify the existing OLE integration test still passes (it doesn't exercise WASAPI directly but proves the PyO3 surface is intact).
* Verify cargo workspace tests still pass.
* Manual TTS responsiveness verification by the user (no automated test possible — would require actual audio hardware and speech synth).

**Out of scope:**

* Changes to `nvda_wasapi` (the inner crate) — its callback type `Box<dyn Fn(u32) + Send>` is the right abstraction and stays.
* The other small perf observations from the audit (extra `GetCurrentPadding` call, byte copy in `feed()`, mutex contention). Those are minor (~μs) and not the user-perceived sluggishness; leave for a follow-up if measurement shows they matter.
* Adding MMCSS thread-priority elevation — neither C++ nor Rust uses it; not regressing parity.
* Refactoring the wider `WasapiPlayer` pyclass — the inline-callback fix is a localized patch.

---

## File Structure

**Modify:**

* `rust/nvda_python/src/wasapi.rs` — replace the `pending_callbacks` field, the `fire_pending_callbacks` helper, and the callback-construction in `WasapiPlayer::new` with an inline GIL-reacquiring closure. Remove the `self.fire_pending_callbacks(py)?;` calls in `feed`, `sync`, and `idle`.

**No other files change.** The inner `nvda_wasapi` crate is unchanged. No new crates, no new tests (the change is fundamentally about FFI behavior and cannot be unit-tested without real audio hardware + a TTS engine).

---

## Working assumptions

1. **`Py<PyAny>` is `Send + Sync`** in PyO3 0.28 — confirmed by the existing wrapper holding `callback: Py<PyAny>` as a struct field that gets accessed across thread boundaries during `py.detach`. Moving it into a `Box<dyn Fn(u32) + Send>` closure (via `clone_ref`) is sound.
2. **`Python::attach(|py| ...)`** is the PyO3 0.28 API for re-acquiring the GIL inside a previously-detached scope. It blocks until the GIL is available and runs the closure with a fresh `Python<'_>` token.
3. **Calling `Py<PyAny>::call1` while holding the GIL** is the standard PyO3 way to invoke a Python callable from Rust. Errors return `PyErr`; we swallow them with `let _ =` and a `log::warn!` to match the spirit of the C++ behavior (which had no error propagation either — Python exceptions in WINFUNCTYPE callbacks just got set as thread state and got cleared by the next normal Python boundary).
4. **GIL re-acquisition cost is negligible.** ~1–5 μs per acquire/release. Even at 100 callbacks/sec (vastly exceeding any realistic TTS callback rate), that's < 0.5 ms total per second. Compare to the 100+ ms `WaitForSingleObject` we're sitting behind today.
5. **The inner crate's `feed_ends.retain(|...| { callback(id); ... })`** invokes the closure directly. If our closure now re-acquires the GIL, the inner crate doesn't care — it just sees a `Fn(u32) -> ()` taking a few μs longer. No inner-crate change needed.

---

## Task 1: Replace deferred callback firing with inline GIL re-acquisition

**Files:**

* Modify: `rust/nvda_python/src/wasapi.rs`

* [ ] **Step 1: Read the current callback construction**

```
sed -n '85,150p' rust/nvda_python/src/wasapi.rs
```

The relevant pieces:

* Line 86–92: `WasapiPlayer` struct with `callback: Py<PyAny>` and `pending_callbacks: Arc<Mutex<Vec<u32>>>`.
* Line 94–106: `fire_pending_callbacks` helper — drains the Vec and calls Python.
* Line 110–150: `WasapiPlayer::new` — constructs an inner-crate callback closure that pushes to `pending_callbacks`.

We're collapsing all of this into a single inline closure that re-acquires the GIL and calls the Python callback directly.

* [ ] **Step 2: Add the `log` dependency to nvda_python (if not already there)**

Check:

```
grep "^log =" rust/nvda_python/Cargo.toml
```

Expected: `log = "0.4"` already present (added in the pyo3-log work). If absent, add it under `[dependencies]`.

* [ ] **Step 3: Replace the `WasapiPlayer` struct definition**

In `rust/nvda_python/src/wasapi.rs`, find:

```rust
#[pyclass]
pub struct WasapiPlayer {
    inner: Mutex<WasapiPlayerInner>,
    stop_handle: StopHandle,
    callback: Py<PyAny>,
    pending_callbacks: Arc<Mutex<Vec<u32>>>,
}

impl WasapiPlayer {
    /// Drain pending feed IDs and call the Python callback for each.
    fn fire_pending_callbacks(&self, py: Python<'_>) -> PyResult<()> {
        let ids: Vec<u32> = {
            let mut pending = self.pending_callbacks.lock().unwrap();
            pending.drain(..).collect()
        };
        for id in ids {
            self.callback.call1(py, (id,))?;
        }
        Ok(())
    }
}
```

Replace with:

```rust
#[pyclass]
pub struct WasapiPlayer {
    inner: Mutex<WasapiPlayerInner>,
    stop_handle: StopHandle,
    /// Kept for `Drop` semantics — the inner-crate callback closure also holds
    /// a clone, but we keep the original here so dropping the WasapiPlayer
    /// drops both references.
    _callback: Py<PyAny>,
}
```

(We also drop the `impl WasapiPlayer { fn fire_pending_callbacks ... }` block — no manual draining needed once callbacks fire inline.)

* [ ] **Step 4: Replace the callback construction in `WasapiPlayer::new`**

Find:

```rust
let pending_callbacks: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
let pending_clone = pending_callbacks.clone();
let callback_fn: Box<dyn Fn(u32) + Send> = Box::new(move |feed_id: u32| {
    pending_clone.lock().unwrap().push(feed_id);
});

let inner = WasapiPlayerInner::new(
    endpointId,
    channels,
    samplesPerSec,
    bitsPerSample,
    Some(callback_fn),
    counters,
)
.map_err(to_os_error)?;

let stop_handle = inner.stop_handle();

Ok(Self {
    inner: Mutex::new(inner),
    stop_handle,
    callback,
    pending_callbacks,
})
```

Replace with:

```rust
// Wrap the Python callable in a Send closure that briefly re-acquires the
// GIL to invoke it. This matches the C++ WINFUNCTYPE behavior: the callback
// fires immediately inside the WASAPI feed loop (between buffer writes) so
// onDone notifications -- e.g. synthIndexReached for TTS -- arrive without
// the ~100ms feed-loop-wait latency the previous queue-and-drain pattern
// added.
//
// Errors from the Python callback are swallowed with a debug log; matches the
// C++ behavior, which had no error-propagation path either (Python exceptions
// raised in WINFUNCTYPE callbacks became thread state that got cleared at the
// next normal Python boundary).
let callback_for_inner = callback.clone_ref(py);
let callback_fn: Box<dyn Fn(u32) + Send> = Box::new(move |feed_id: u32| {
    Python::attach(|py| {
        if let Err(e) = callback_for_inner.call1(py, (feed_id,)) {
            log::warn!(
                "WasapiPlayer feed-done callback raised: {e:?} (feed_id={feed_id})",
            );
        }
    });
});

let inner = WasapiPlayerInner::new(
    endpointId,
    channels,
    samplesPerSec,
    bitsPerSample,
    Some(callback_fn),
    counters,
)
.map_err(to_os_error)?;

let stop_handle = inner.stop_handle();

Ok(Self {
    inner: Mutex::new(inner),
    stop_handle,
    _callback: callback,
})
```

**Note:** The `new` function already takes `py: Python<'_>` implicitly (via the `&pyo3::Bound<'_, PyModule>` for the constructor). If the existing signature doesn't have access to `py`, add a `py: Python<'_>` parameter — PyO3 0.28's `#[new]` macro accepts it as the first parameter. Check `Py<PyAny>::clone_ref(&self, py: Python<'_>)` — needs the GIL token to bump the Python refcount. If you can't get `py` from the constructor context, fall back to `Python::attach(|py| callback.clone_ref(py))` inside the body.

**Verify** the constructor still has the right signature by reading lines 110–125 of `rust/nvda_python/src/wasapi.rs`. The current constructor doesn't take `py` — see:

```rust
#[new]
#[pyo3(signature = (endpointId, channels, samplesPerSec, bitsPerSample, callback))]
fn new(
    endpointId: &str,
    channels: u16,
    samplesPerSec: u32,
    bitsPerSample: u16,
    callback: Py<PyAny>,
) -> PyResult<Self> {
```

You'll need to add `py: Python<'_>` as the first parameter (PyO3 special-cases it):

```rust
#[new]
#[pyo3(signature = (endpointId, channels, samplesPerSec, bitsPerSample, callback))]
fn new(
    py: Python<'_>,
    endpointId: &str,
    channels: u16,
    samplesPerSec: u32,
    bitsPerSample: u16,
    callback: Py<PyAny>,
) -> PyResult<Self> {
```

PyO3 will automatically supply the GIL token; `endpointId` etc. continue to be passed from Python as before.

* [ ] **Step 5: Remove the `self.fire_pending_callbacks(py)?;` calls**

There are three call sites in `feed`, `sync`, and `idle`. Find each and remove only that line — keep everything else.

In `fn feed`:

```rust
fn feed(&self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
    let data_owned = data.to_vec();
    let feed_id;
    {
        // ... (unchanged) ...
        feed_id = py.detach(move || unsafe {
            player_ptr.as_mut().feed(Some(&data_owned), true)
        }).map_err(to_os_error)?;
    }
    self.fire_pending_callbacks(py)?;   // <-- REMOVE THIS LINE
    Ok(feed_id)
}
```

becomes:

```rust
fn feed(&self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
    let data_owned = data.to_vec();
    let feed_id;
    {
        // ... (unchanged) ...
        feed_id = py.detach(move || unsafe {
            player_ptr.as_mut().feed(Some(&data_owned), true)
        }).map_err(to_os_error)?;
    }
    Ok(feed_id)
}
```

The `py: Python<'_>` parameter stays (other code paths might still need it; keeping the signature avoids cross-cutting changes). Same edit in `sync` and `idle` — remove just the `self.fire_pending_callbacks(py)?;` line, leave `py: Python<'_>` as a parameter.

* [ ] **Step 6: Verify it compiles**

```
cd rust && cargo check -p nvda_python 2>&1 | tail -10
```

Expected: clean build, no warnings.

If you see warnings about unused imports — `Arc` and `Mutex<Vec<u32>>` may no longer be needed for the `pending_callbacks` field. Remove the unused parts of `use std::sync::{Arc, Mutex, OnceLock};` if applicable (Mutex is still needed for `inner` and `SILENCE_PLAYER`; Arc may still be used elsewhere — check before removing).

* [ ] **Step 7: Rebuild via SCons (so the .pyd is fresh in .venv)**

```
./scons.bat source --all-cores 2>&1 | tail -3
```

Expected: `scons: done building targets.` and the `.pyd` mtime is "now".

Verify:

```
ls -la .venv/Lib/site-packages/nvdaRust/nvdaRust.cp313-win_amd64.pyd
```

* [ ] **Step 8: Commit**

```bash
git add rust/nvda_python/src/wasapi.rs
git commit -m "nvda_python wasapi: fire feed-done callbacks inline (matches C++ latency)"
```

---

## Task 2: Verify nothing regressed

**Files:** none modified — verification gate.

* [ ] **Step 1: Workspace cargo tests**

```
cd rust && cargo test --workspace 2>&1 | grep "test result" | head -10
```

Expected: same 50+ tests pass; no new failures.

* [ ] **Step 2: Full Python unit suite**

```
./rununittests.bat 2>&1 | tail -5
```

Expected: `Ran 1164+ tests in <T>s, OK`.

* [ ] **Step 3: OLE integration test (proves PyO3 surface is intact)**

```
uv run --no-sync python tests/manual/rust/oleIntegration.py 2>&1 | tail -10
```

Expected: all cases PASS, `Captured 2 total log record(s)` (or similar).

This test doesn't exercise WASAPI but does prove the broader nvdaRust module still loads and the pyo3-log integration is unbroken.

* [ ] **Step 4: No commit** — verification only.

---

## Task 3: Manual TTS responsiveness verification (user-driven)

**Files:** none modified.

Automated testing of perceived TTS responsiveness isn't feasible — needs a real audio device, a real TTS synth, and human ears. The user runs this gate.

* [ ] **Step 1: Start NVDA from source**

```
runnvda.bat
```

(NVDA starts using the freshly-built `.pyd` from Task 1.)

* [ ] **Step 2: Trigger interactive scenarios that previously felt sluggish**

Suggestions:

* Type quickly in a text editor; character echo should keep up.
* Tab through controls in a dialog; focus announcements should be snappy.
* Press `NVDA+up` repeatedly to read previous lines.
* Read a long paragraph (e.g. a wiki article) with say-all; verify word boundary highlighting tracks the speech without lag.

Compare against your subjective baseline from before this branch. Audio should feel as responsive as the C++ reference.

* [ ] **Step 3: Confirm there are no audio glitches**

Drop-outs, stutters, or doubled audio would suggest the inline-callback approach has unintended interaction with the feed loop. None expected — callbacks are short and the GIL re-acquire is brief — but worth listening for.

* [ ] **Step 4: Confirm onDone notifications still fire correctly**

For TTS engines like espeak, NVDA's say-all relies on `synthIndexReached` to know when to fetch the next paragraph. Verify continuous reading flows smoothly from one paragraph to the next, with no gaps from missed callbacks.

* [ ] **Step 5: No commit** — verification only.

If you observe regressions: capture a sample (audio recording, reproducible steps), revert with `git revert <Task 1 commit SHA>`, and escalate.

---

## Task 4: Push

* [ ] **Step 1: Confirm clean tree**

```
git status -s
```

Expected: only the unstaged submodule entries we already know about.

* [ ] **Step 2: Show commit log**

```
git log --oneline origin/master..HEAD | head -5
```

Expected: 1 new commit on top of the SCons-integration work.

* [ ] **Step 3: Push**

```
git push origin HEAD
```

Per project convention, do NOT open a PR. Push and let the user eyeball the diff before opening anything.

---

## Out of scope

* **The other small perf observations** from the audit:
  * Extra `GetCurrentPadding()` call per loop iteration — microseconds; not perceptible.
  * `data.to_vec()` byte copy in PyO3 wrapper — μs per chunk; required for `py.detach` lifetime safety.
  * Mutex around inner player — only matters for concurrent calls, which the typical single-thread synth pattern doesn't have.
  These can be a follow-up audit if measurement shows they matter.

* **MMCSS thread-priority elevation** — neither C++ nor Rust uses it; this plan preserves that parity. Adding MMCSS would be a separate optimization that may or may not help (would need measurement).

* **Refactoring `WasapiPlayer` more broadly** — keep the patch localized to the callback path. Other concerns (mutex granularity, internal state organization) can be addressed independently.

* **A regression test** — would require building a fake `WasapiPlayerInner` that doesn't touch real audio, plus a way to time callback firing relative to feed return. Doable but high cost for a one-off behavior fix that's manually verifiable.
