# Rust WASAPI Layer for NVDA — Design

## Goal

Replace the C++ WASAPI implementation (`wasapi.cpp`) with Rust, covering both
`WasapiPlayer` and `SilencePlayer`. This removes all C++ audio playback code,
leaving only Rust for audio and the existing Python orchestration layer.

## Architecture

```
nvwave.py  ──→  nvdaRust.wasapi.WasapiPlayer (PyO3 #[pyclass])
    │           nvdaRust.wasapi.silenceInit / silencePlayFor / silenceTerminate
    │
    └──→  audioDucking.py (unchanged, still Python)
```

The Rust `nvda_wasapi` crate wraps `windows-rs` COM calls for WASAPI.
`nvda_python` re-exports it as `nvdaRust.wasapi`. The Python `nvwave.py`
creates Rust objects instead of calling `wasPlay_*` ctypes functions.

## Workspace Changes

```
rust/
├── Cargo.toml                  # add nvda_wasapi to members
├── nvda_core/                  # unchanged
├── nvda_tones/                 # unchanged
├── nvda_wasapi/                # NEW
│   ├── Cargo.toml              # depends on nvda_core + windows-rs
│   └── src/
│       ├── lib.rs              # module root
│       ├── player.rs           # WasapiPlayer struct + impl
│       ├── silence.rs          # SilencePlayer (background thread)
│       ├── device.rs           # device enumeration, fallback, notification client
│       └── silence_detect.rs   # leading silence trimming
└── nvda_python/
    ├── Cargo.toml              # add nvda_wasapi dependency
    └── src/lib.rs              # add wasapi submodule
```

## Python API (camelCase)

### WasapiPlayer

```python
import nvdaRust

# Constructor — takes individual format params, not WAVEFORMATEX struct
player = nvdaRust.wasapi.WasapiPlayer(
    endpointId="",           # empty string = default device
    channels=2,
    samplesPerSec=44100,
    bitsPerSample=16,
    callback=on_feed_done,   # Python callable: callback(feedId: int)
)

player.open()                                    # open/reopen device
feedId = player.feed(data: bytes)                # feed PCM, returns feedId (int)
player.stop()                                    # stop playback
player.sync()                                    # block until playback finishes
player.idle()                                    # sync + stop + reset
player.pause()                                   # pause stream
player.resume()                                  # resume stream
player.setChannelVolume(channel: int, level: float)
player.startTrimmingLeadingSilence(start: bool)
```

### SilencePlayer

```python
nvdaRust.wasapi.silenceInit(endpointId: str)
nvdaRust.wasapi.silencePlayFor(ms: int, volume: float)
nvdaRust.wasapi.silenceTerminate()
nvdaRust.wasapi.wasapiStartup()                  # COM init, register notifications
```

### Key API differences from C++

1. **No opaque pointer.** `WasapiPlayer` is a PyO3 `#[pyclass]` — Python holds the
   object directly, destructor runs automatically.
2. **`feed()` returns `feedId`** (an `int`) instead of writing to a pointer. If no
   callback is needed, the caller ignores the return value.
3. **Callback is a Python callable** — no CFUNCTYPE needed. Rust acquires GIL to call it.
4. **Format as individual params** — no WAVEFORMATEX struct crossing the boundary.
5. **Errors as Python exceptions** — HRESULT failures become `OSError` with `winerror`.

## Rust Implementation Details

### WasapiPlayer struct

```rust
#[pyclass]
pub struct WasapiPlayer {
    client: Option<IAudioClient>,
    render: Option<IAudioRenderClient>,
    clock: Option<IAudioClock>,
    buffer_frames: u32,
    endpoint_id: String,
    format: WAVEFORMATEX,
    callback: PyObject,              // Python callable
    play_state: PlayState,
    feed_ends: Vec<(u32, u64)>,      // (feedId, endTimeMs)
    clock_freq: u64,
    sent_frames: u32,
    next_feed_id: u32,
    wake_event: HANDLE,
    default_device_change_count: u32,
    device_state_change_count: u32,
    is_using_preferred_device: bool,
    is_trimming_leading_silence: bool,
}
```

### GIL Handling (critical)

| Method | GIL behavior |
|--------|-------------|
| `open()` | Hold GIL (fast COM calls) |
| `feed()` | Release GIL during `WaitForSingleObject`; reacquire for callback |
| `sync()` | Release GIL during wait loop; reacquire for callbacks |
| `idle()` | Calls sync() then stop() |
| `stop()` | Hold GIL (fast) |
| `pause()` / `resume()` | Hold GIL (fast) |
| `setChannelVolume()` | Hold GIL (fast) |

Pattern for `feed()`:
```rust
fn feed(&mut self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
    // ... setup ...
    py.allow_threads(|| {
        // WaitForSingleObject, GetCurrentPadding, GetBuffer, ReleaseBuffer
    });
    // Reacquire GIL for callback
    self.maybe_fire_callbacks(py)?;
    Ok(feed_id)
}
```

### Device Management

Same strategy as C++:
- `IMMNotificationClient` implementation in Rust (COM trait via `windows-rs`)
- Global notification client registered once via `wasapiStartup()`
- Change counters polled in `feed()` to detect device changes
- Preferred device → default device fallback
- `open(force)` reopens when device changes detected

### SilencePlayer

Background `std::thread` (no GIL involvement):
- Owns a Rust-level `WasapiPlayer` (not the PyO3 wrapper — a separate internal struct)
- Generates white noise from normal distribution
- Wakes via `Event` when `silencePlayFor()` is called
- Self-destructs on `silenceTerminate()`

This means the core WASAPI logic exists as an internal Rust struct, and the `#[pyclass]`
wrapper adds PyO3 bindings + GIL handling on top. SilencePlayer uses the internal struct
directly without PyO3 overhead.

### Internal vs PyO3 Split

```
nvda_wasapi/src/player.rs:
  struct WasapiPlayerInner { ... }  // Pure Rust, no PyO3
  impl WasapiPlayerInner {
      fn open(), feed(), stop(), sync(), ...
  }

nvda_python/src/wasapi.rs:
  #[pyclass]
  struct WasapiPlayer {
      inner: WasapiPlayerInner,
      callback: PyObject,
  }
  #[pymethods]
  impl WasapiPlayer {
      fn feed(&mut self, py: Python, data: &[u8]) -> PyResult<u32> {
          py.allow_threads(|| self.inner.feed(data))
          // ... fire callbacks with GIL ...
      }
  }

nvda_wasapi/src/silence.rs:
  struct SilencePlayer {
      player: WasapiPlayerInner,  // Uses internal struct directly
      thread: JoinHandle<()>,
  }
```

### windows-rs Features Needed

```toml
[dependencies.windows]
version = "0.58"
features = [
    "Win32_Media_Audio",
    "Win32_System_Com",
    "Win32_System_Threading",
    "Win32_Foundation",
    "Win32_Devices_FunctionDiscovery",
]
```

Note: Check latest `windows` crate version at build time.

## Changes to nvwave.py

### Constructor

```python
# Before:
self._player = wasapi.wasPlay_create(outputDevice, format, WavePlayer._callback)
self._instances[self._player] = self

# After:
import nvdaRust
self._player = nvdaRust.wasapi.WasapiPlayer(
    outputDevice, channels, samplesPerSec, bitsPerSample,
    self._makeFeedCallback(),
)
```

The `_instances` WeakValueDictionary and static `_callback` with CFUNCTYPE are no
longer needed — each Python WavePlayer holds its Rust WasapiPlayer directly, and
the callback is a regular Python callable (closure or bound method).

### feed()

```python
# Before:
feedId = c_uint() if onDone else None
wasapi.wasPlay_feed(self._player, data, size, byref(feedId) if onDone else None)
if onDone:
    self._doneCallbacks[feedId.value] = onDone

# After:
feedId = self._player.feed(data)
if onDone:
    self._doneCallbacks[feedId] = onDone
```

### Other methods

Direct method calls replace ctypes function calls:
```python
# Before:                              # After:
wasapi.wasPlay_open(self._player)      self._player.open()
wasapi.wasPlay_stop(self._player)      self._player.stop()
wasapi.wasPlay_sync(self._player)      self._player.sync()
wasapi.wasPlay_idle(self._player)      self._player.idle()
wasapi.wasPlay_pause(self._player)     self._player.pause()
wasapi.wasPlay_resume(self._player)    self._player.resume()
```

### Silence calls

```python
# Before:                                      # After:
wasapi.wasSilence_init(outputDevice)            nvdaRust.wasapi.silenceInit(outputDevice)
wasapi.wasSilence_playFor(ms, volume)           nvdaRust.wasapi.silencePlayFor(ms, volume)
wasapi.wasSilence_terminate()                   nvdaRust.wasapi.silenceTerminate()
```

### initialize() / terminate()

```python
# Before:
wasapi.wasPlay_startup()

# After:
nvdaRust.wasapi.wasapiStartup()
```

The `initialize()` function simplifies significantly — no more setting `restype = HRESULT`
on each function since PyO3 handles error conversion automatically.

### __del__

```python
# Before:
wasapi.wasPlay_destroy(self._player)

# After:
# Rust Drop trait handles cleanup automatically when self._player goes out of scope.
# No explicit destroy call needed.
```

## What Changes

| Component | Change |
|-----------|--------|
| `rust/nvda_wasapi/` | New crate — full WASAPI implementation |
| `rust/nvda_python/` | Re-exports `nvdaRust.wasapi` submodule |
| `source/nvwave.py` | Switches from ctypes to `nvdaRust.wasapi` |
| `source/wasapi.py` | Can be removed (all wasPlay_* bindings unused) |
| `nvdaHelper/local/wasapi.cpp` | Stays in tree but unused by Python |

## What Doesn't Change

| Component | Status |
|-----------|--------|
| `audioDucking.py` | Untouched |
| `nvwave.py` WavePlayer class structure | Same — idle management, ducking, config |
| `playWaveFile()` | Untouched (uses WavePlayer) |
| `tones.py` | Untouched (uses nvdaRust.tones + WavePlayer) |
| Add-on API | Untouched |

## Testing Strategy

### Rust unit tests (no audio hardware needed)

- PlayState state machine transitions
- Buffer frame calculations
- Feed ID tracking and callback ordering
- Silence detection (leading silence trimming)
- Device fallback logic (mock device enumerator)

### Python integration tests

- Create WasapiPlayer, feed PCM, verify no errors
- Verify callback fires with correct feedId
- Verify pause/resume
- Verify volume setting
- Verify silence player init/playFor/terminate

### Manual testing

- Run NVDA, verify speech plays correctly
- Verify device hot-swap (unplug/replug audio device)
- Verify audio ducking still works
- Verify idle timeout (stream closes after 10s)
- Verify leading silence trimming for speech
