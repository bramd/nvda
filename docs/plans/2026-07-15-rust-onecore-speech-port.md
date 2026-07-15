# Porting the OneCore speech engine to Rust

**Status:** plan (2026-07-15). Survey complete (C++ engine read + agent map of
the Python driver + windows-rs coverage confirmed). The main WinRT port, on
the default synth path; reuses the pattern proven by the uwpOcr warm-up
(`2026-07-14-...`). Completes the Rust audio pipeline: the synth engine feeds
`WavePlayer` → the already-Rust `WasapiPlayer`.

## What it is

`nvdaHelper/localWin10/oneCoreSpeech.cpp` — a C++/WinRT bridge to
`Windows.Media.SpeechSynthesis`, exposing **18 `__stdcall` `ocSpeech_*`
functions** driven by `source/synthDrivers/oneCore.py` (ctypes `windll`).
`speak` is async: it synthesizes an utterance to a stream on a background
thread and delivers, in **one callback per utterance**,
`(BYTE* wavBuffer, int length, const wchar_t* markers)` — a full WAV buffer +
a `"text:time|text:time|…"` markers string (time in 100 ns ticks). Python
strips the 46-byte WAV header, feeds PCM to the Rust `WasapiPlayer`, and
splits at marker boundaries to fire bookmark/index notifications. Everything
downstream stays Python.

## Why it's meatier than uwpOcr (a state machine, not just a wrapper)

* **Token/activation model:** `initialize` creates a `SpeechSynthesizer` and
  returns an opaque token (the synth identity); every call validates it to
  reject use-after-`terminate` and stale async callbacks.
* **A `shared_timed_mutex` read/write protocol:** `initialize`/`terminate`
  take the **write** lock; the async callback takes a **read** lock,
  re-validates the token under it, and fires — so `terminate` blocks while a
  callback is in flight and a callback never fires for a terminated synth.
* 16 more accessors (voices, current voice id/language, rate/pitch/volume,
  punctuation silence, two `ApiInformation` feature gates).

windows-rs makes all of this *cleaner* than the C++: `RwLock` for the
`shared_timed_mutex`, `Arc` for the `shared_ptr` lifecycle, no coroutines.
Coverage confirmed: `SpeechSynthesizer` / `SpeechSynthesisStream` /
`VoiceInformation` / `SpeechSynthesizerOptions` + `SynthesizeSsmlToStreamAsync`
/ `AllVoices` / `Voice` / `Options` / `Markers` / `AudioPitch` / `AudioVolume`
/ `SpeakingRate` / `AppendedSilence` / `PunctuationSilence`.

## Design decisions

**D1 — Token = generation counter, not a raw pointer.** Global
`RwLock<Option<Active>>` with `Active { generation: u64, synth, callback }`
and an `AtomicU64` generation (start at 1; 0/NULL = never valid). `initialize`
bumps the generation, stores `Active`, returns the generation as the opaque
`void*` token. Cleaner and reuse-safe vs. the C++ raw-pointer token.

**D2 — `RwLock` reproduces the read/write protocol.** `initialize`/`terminate`
take the write lock; getters/`speak`/the async callback take the read lock.
The slow WinRT async runs **outside** the lock: `speak` snapshots
`(generation, synth.clone(), callback)` under a brief read lock, releases,
does `SynthesizeSsmlToStreamAsync().get()` + `ReadAsync().get()` on a
background thread with no lock held, then re-acquires the read lock,
re-validates the generation, and fires the callback under it — exactly the
C++ `protectedCallback_` discipline.

**D3 — `unsafe impl Send + Sync` for the WinRT synth.** It lives in the
global state and is used from both the Python thread and background callback
threads; WinRT synth objects are agile (the C++ shares one via `shared_ptr`
across the GIL thread + threadpool). Documented, as in uwpOcr's D2.

**D4 — Thread-local buffer for `getCurrentVoiceId`/`Language`.** The C++
returns `c_str()` of a *local* `hstring` (a dangling pointer that only works
because the caller copies immediately). Rust backs these with a thread-local
`Vec<u16>` holding the last value until the next call — safe, same "read
immediately" contract.

**D5 — `speak` async = background thread + blocking `.get()`** (the uwpOcr
pattern). Read the whole `SpeechSynthesisStream` into an `IBuffer`, take its
bytes via `IBufferByteAccess`, build the markers string from
`stream.Markers()` (`IMediaMarker.Text()` + `.Time().Duration`), fire
`callback(data, len, markers)` — or `(null, 0, null)` on error.

**D6 — Preserve the exact string formats:** voices
`"Id:Language:DisplayName|…"` BSTR (freed by the Python caller), markers
`"text:time|…"`, and `AppendedSilence::Min` on init (gated on
`ApiInformation` contract 6.0).

## Phased plan (build + test each)

* **Phase 1 — crate + state machine + accessors (no audio).** New crate
  `nvda_onecore_speech`. Confirm windows-rs `Media_SpeechSynthesis` +
  `Media_Core` (`IMediaMarker`) + `Foundation_Metadata` (`ApiInformation`)
  compile. Implement the `RwLock`/generation state, `initialize`/`terminate`,
  `getVoices`, `getCurrentVoiceId`/`Language`, `setVoice`, get/set
  rate/pitch/volume, `supports*`, punctuation silence. Standalone-testable:
  init → getVoices → getCurrentVoiceId → set/get rate.
* **Phase 2 — `speak` + async + callback.** The background-thread synthesis,
  WAV buffer read, markers, and callback. Standalone-testable: `init(cb)` →
  `speak("<speak>…</speak>")` → callback fires with WAV bytes + markers;
  parse the WAV header + markers to sanity-check.
* **Phase 3 — flip + build.** The build plumbing already exists (uwpOcr added
  the archBuild `_localRustLibs` dict + the localWin10 Rust link): add
  `nvda_onecore_speech` to the dict, `/EXPORT` its 18 symbols, delete
  `oneCoreSpeech.cpp`/`.h`. Build x64 `nvdaHelperLocalWin10.dll`, dumpbin the
  exports, standalone smoke test. **Then the user smoke-tests actual speech
  in NVDA** (select OneCore, hear it, check rate/voice, bookmarks). With this,
  `nvdaHelperLocalWin10.dll` is 100 % Rust.

## Risks

* WinRT `IMediaMarker` (`Media_Core`) + `ApiInformation` (`Foundation_Metadata`)
  feature coverage — verify at Phase 1 compile (low risk; the methods exist).
* The DLL will have no C++ source once both engines are Rust (only the `.res`
  * two staticlibs); the MSVC CRT still provides `_DllMainCRTStartup`. Verify
  it still links as a DLL at Phase 3 (very low risk).
* Behaviour is speech — the standalone tests validate the ABI + audio bytes,
  but real-voice quality/latency/bookmarks need the in-NVDA smoke test.
