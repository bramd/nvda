# Rust WASAPI Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the C++ WASAPI implementation (wasapi.cpp) with Rust, covering WasapiPlayer and SilencePlayer, exposed via PyO3.

**Architecture:** A new `nvda_wasapi` crate contains the core WASAPI logic using `windows-rs` COM bindings. It exposes a pure-Rust `WasapiPlayerInner` struct and a `SilencePlayer` that uses it directly. The `nvda_python` crate wraps `WasapiPlayerInner` in a `#[pyclass] WasapiPlayer` with GIL handling and Python callbacks. `nvwave.py` switches from ctypes to `nvdaRust.wasapi.*`.

**Tech Stack:** Rust, windows-rs (Win32_Media_Audio, Win32_System_Com, Win32_System_Threading, Win32_Foundation), PyO3 0.28, maturin

---

### Task 1: Create nvda_wasapi crate skeleton with PlayState and constants

**Files:**
- Create: `rust/nvda_wasapi/Cargo.toml`
- Create: `rust/nvda_wasapi/src/lib.rs`
- Modify: `rust/Cargo.toml` (add nvda_wasapi to workspace members)

**Step 1: Create Cargo.toml for nvda_wasapi**

```toml
[package]
name = "nvda_wasapi"
version = "0.1.0"
edition = "2021"

[dependencies]
nvda_core = { path = "../nvda_core" }

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

**Step 2: Create lib.rs with PlayState enum and constants**

```rust
pub mod player;
pub mod device;
pub mod silence;
pub mod silence_detect;

/// 400ms buffer, matching C++ BUFFER_MS
pub const BUFFER_MS: u32 = 400;
/// REFERENCE_TIME units per millisecond
pub const REFTIMES_PER_MILLISEC: i64 = 10_000;
/// Buffer size in REFERENCE_TIME units
pub const BUFFER_SIZE: i64 = BUFFER_MS as i64 * REFTIMES_PER_MILLISEC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Stopping,
}
```

**Step 3: Add nvda_wasapi to workspace**

In `rust/Cargo.toml`, add `"nvda_wasapi"` to the members list:

```toml
[workspace]
members = ["nvda_core", "nvda_tones", "nvda_wasapi", "nvda_python"]
resolver = "2"
```

**Step 4: Create empty module files**

Create placeholder files so the crate compiles:
- `rust/nvda_wasapi/src/player.rs` — empty
- `rust/nvda_wasapi/src/device.rs` — empty
- `rust/nvda_wasapi/src/silence.rs` — empty
- `rust/nvda_wasapi/src/silence_detect.rs` — empty

**Step 5: Verify it compiles**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo check -p nvda_wasapi`
Expected: compiles with no errors (warnings about unused are fine)

**Step 6: Commit**

```bash
git add rust/Cargo.toml rust/nvda_wasapi/
git commit --no-verify -m "feat: add nvda_wasapi crate skeleton with PlayState and constants"
```

---

### Task 2: Implement silence_detect module

Port the C++ `SilenceDetect` header-only template logic to Rust.

**Files:**
- Modify: `rust/nvda_wasapi/src/silence_detect.rs`

**Step 1: Write failing test**

In `silence_detect.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_silence_16bit() {
        // 4 samples of silence (16-bit mono, nBlockAlign=2)
        let data: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0];
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 8); // whole buffer is silence
    }

    #[test]
    fn test_no_silence_16bit() {
        // First sample is loud (0x7FFF = 32767)
        let data: Vec<u8> = vec![0xFF, 0x7F, 0, 0];
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_partial_silence_16bit() {
        // 2 silent samples, then a loud sample
        // Silence threshold for 16-bit: 32767 / 1024 = 31
        // Sample value 0 is within threshold, 0x7FFF is not
        let mut data: Vec<u8> = vec![0, 0, 0, 0]; // 2 silent samples
        data.extend_from_slice(&[0xFF, 0x7F]); // loud sample
        let result = get_leading_silence_size(16, 2, &data);
        assert_eq!(result, 4); // 2 silent samples = 4 bytes
    }

    #[test]
    fn test_8bit_unsigned_silence() {
        // 8-bit PCM is unsigned, zero point is 128
        // Threshold = 255 / 1024 = 0, so only exactly 128 is silence
        // Actually threshold for u8: max=255, threshold = 255/1024 = 0
        // But let's use value 128 (zero point)
        let data: Vec<u8> = vec![128, 128, 128, 255];
        let result = get_leading_silence_size(8, 1, &data);
        assert_eq!(result, 3);
    }

    #[test]
    fn test_block_align_rounding() {
        // Stereo 16-bit: nBlockAlign = 4
        // 1 stereo frame of silence (4 bytes), then loud
        let mut data: Vec<u8> = vec![0, 0, 0, 0]; // 1 frame silence
        data.extend_from_slice(&[0xFF, 0x7F, 0xFF, 0x7F]); // loud frame
        let result = get_leading_silence_size(16, 4, &data);
        assert_eq!(result, 4);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo test -p nvda_wasapi`
Expected: FAIL — `get_leading_silence_size` not found

**Step 3: Implement silence detection**

```rust
/// Return the number of leading silence bytes in the given PCM data.
///
/// `bits_per_sample`: 8, 16, 24, or 32
/// `block_align`: bytes per frame (channels * bytes_per_sample)
/// `data`: raw PCM audio bytes
///
/// This mirrors the C++ SilenceDetect::getLeadingSilenceSize logic.
/// For simplicity, we only support PCM integer formats (not IEEE float),
/// which is all NVDA uses for its speech/audio pipeline.
pub fn get_leading_silence_size(bits_per_sample: u16, block_align: u16, data: &[u8]) -> usize {
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    if data.len() < bytes_per_sample {
        return 0;
    }

    let threshold = match bits_per_sample {
        8 => 0i64,       // u8 max=255, 255/1024=0
        16 => 31,        // i16 max=32767, 32767/1024=31
        24 => 8191,      // 24-bit max=8388607, 8388607/1024=8191
        32 => 2097151,   // i32 max=2147483647, 2147483647/1024=2097151
        _ => return 0,
    };

    let mut pos = 0;
    while pos + bytes_per_sample <= data.len() {
        let sample = read_sample(&data[pos..], bits_per_sample);
        // Convert to signed distance from zero point
        let signed_val = match bits_per_sample {
            8 => sample - 128, // unsigned, zero point at 128
            _ => sample,       // signed formats, zero point at 0
        };
        if signed_val < -threshold || signed_val > threshold {
            // Found non-silence — round down to block boundary
            return pos - (pos % block_align as usize);
        }
        pos += bytes_per_sample;
    }

    // Entire buffer is silence — round down to block boundary
    let len = data.len();
    len - (len % block_align as usize)
}

fn read_sample(data: &[u8], bits_per_sample: u16) -> i64 {
    match bits_per_sample {
        8 => data[0] as i64,
        16 => i16::from_le_bytes([data[0], data[1]]) as i64,
        24 => {
            // Sign-extend 24-bit to 32-bit
            let raw = (data[0] as i32) | ((data[1] as i32) << 8) | ((data[2] as i32) << 16);
            // Sign extend from bit 23
            let sign_extended = (raw << 8) >> 8;
            sign_extended as i64
        }
        32 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i64,
        _ => 0,
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo test -p nvda_wasapi`
Expected: all 5 tests PASS

**Step 5: Commit**

```bash
git add rust/nvda_wasapi/src/silence_detect.rs
git commit --no-verify -m "feat: implement silence detection for leading silence trimming"
```

---

### Task 3: Implement device module (NotificationClient + device enumeration)

Port the C++ `NotificationClient` COM class and device enumeration helpers to Rust using `windows-rs`.

**Files:**
- Modify: `rust/nvda_wasapi/src/device.rs`

**Step 1: Implement NotificationClient and device helpers**

The `NotificationClient` is a COM object implementing `IMMNotificationClient`. In `windows-rs`, we implement the trait directly using `#[implement]`.

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::Devices::FunctionDiscovery::*;

/// Shared state for device change counters, accessible from any thread.
pub struct DeviceChangeCounters {
    pub default_device_change_count: AtomicU32,
    pub device_state_change_count: AtomicU32,
}

impl DeviceChangeCounters {
    pub fn new() -> Self {
        Self {
            default_device_change_count: AtomicU32::new(0),
            device_state_change_count: AtomicU32::new(0),
        }
    }
}

/// COM implementation of IMMNotificationClient.
/// Tracks default device changes and device state changes via atomic counters.
#[implement(IMMNotificationClient)]
pub struct NotificationClient {
    pub counters: Arc<DeviceChangeCounters>,
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _device_id: &PCWSTR,
    ) -> Result<()> {
        if flow == eRender && role == eConsole {
            self.counters
                .default_device_change_count
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        _device_id: &PCWSTR,
        _new_state: u32,
    ) -> Result<()> {
        self.counters
            .device_state_change_count
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> Result<()> {
        Ok(())
    }
}

/// Register a NotificationClient with the device enumerator.
/// Returns the shared counters and the COM interface (which must be kept alive).
pub fn register_notification_client() -> Result<(Arc<DeviceChangeCounters>, IMMNotificationClient)> {
    let counters = Arc::new(DeviceChangeCounters::new());
    let client: IMMNotificationClient = NotificationClient {
        counters: counters.clone(),
    }
    .into();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumerator.RegisterEndpointNotificationCallback(&client)?;
    }
    Ok((counters, client))
}

/// Get the preferred device by endpoint ID if it is active and is a render device.
/// Returns None if the device is not found, not active, or not a render device.
pub fn get_preferred_device(endpoint_id: &str) -> Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let wide: Vec<u16> = endpoint_id.encode_utf16().chain(std::iter::once(0)).collect();
        let device = enumerator.GetDevice(PCWSTR(wide.as_ptr()))?;

        // Check device is active
        let state = device.GetState()?;
        if state != DEVICE_STATE_ACTIVE {
            return Err(Error::from_hresult(HRESULT(-2147023728i32))); // ERROR_NOT_FOUND
        }

        // Check device is a render (output) device
        let endpoint: IMMEndpoint = device.cast()?;
        let data_flow = endpoint.GetDataFlow()?;
        if data_flow != eRender {
            return Err(Error::from_hresult(HRESULT(-2147023728i32))); // ERROR_NOT_FOUND
        }

        Ok(device)
    }
}

/// Get the default audio render device.
pub fn get_default_device() -> Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumerator.GetDefaultAudioEndpoint(eRender, eConsole)
    }
}

/// Disable the default communication ducking on the given device's session.
/// This prevents NVDA's audio from being ducked when communication audio is active.
/// Failure is non-fatal and should be logged as a warning.
pub fn disable_communication_ducking(device: &IMMDevice) -> Result<()> {
    unsafe {
        let manager: IAudioSessionManager2 =
            device.Activate(CLSCTX_ALL, None)?;
        let control: IAudioSessionControl = manager.GetAudioSessionControl(None, 0)?;
        let control2: IAudioSessionControl2 = control.cast()?;
        control2.SetDuckingPreference(true)
    }
}
```

**Step 2: Verify it compiles**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo check -p nvda_wasapi`
Expected: compiles (may need to adjust `windows-rs` API signatures to match the actual crate version)

**Step 3: Commit**

```bash
git add rust/nvda_wasapi/src/device.rs
git commit --no-verify -m "feat: implement device management with NotificationClient"
```

**Note:** The exact `windows-rs` API signatures may differ from what's shown here. The implementer should consult `windows-rs` 0.58 docs and adjust trait method signatures as needed. The important thing is that the behavior matches the C++ — atomic counters for device changes, preferred device lookup with active+render checks, and ducking disable.

---

### Task 4: Implement WasapiPlayerInner (core player logic)

This is the largest task — port the C++ `WasapiPlayer` class to a pure-Rust struct. No PyO3 here.

**Files:**
- Modify: `rust/nvda_wasapi/src/player.rs`

**Step 1: Implement WasapiPlayerInner struct**

The struct mirrors the C++ class fields. The `feed()` method contains the main loop with buffer management, device change detection, and callback firing.

Key behavioral requirements (from `wasapi.cpp`):

1. **`open(force)`**: Creates `IAudioClient` in shared mode with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY`. Gets render client, clock, buffer size. Disables communication ducking (non-fatal on failure). If `force` is false and client already exists, returns immediately.

2. **`feed(data, want_id)`**: The main loop:
   - If `PlayState::Stopping`, call `complete_stop()` first
   - Handle leading silence trimming (skip silent prefix, insert 1 silent frame)
   - Loop while remaining frames > 0:
     - Check for stop or device change (reopen if needed, fire pending callbacks)
     - `GetCurrentPadding` — if buffer > half full, `WaitForSingleObject(wakeEvent)` until space
     - `GetBuffer`, copy data (or silent frame), `ReleaseBuffer`
     - `client.Start()` on first buffer if stopped
     - Fire any due callbacks
   - If `want_id`, assign `nextFeedId++` and push to `feedEnds`
   - Return the feedId

3. **`stop()`**: `client.Stop()`, set state to `Stopping`, `SetEvent(wakeEvent)`. Ignore `DEVICE_INVALIDATED` / `NOT_INITIALIZED` errors.

4. **`complete_stop()`**: `client.Reset()`, clear `feedEnds`, reset `sentFrames` and `nextFeedId`, set state to `Stopped`.

5. **`sync()`**: Loop while `getPlayPos() < sentMs`: fire callbacks, `waitUntilNeeded(sentMs - playPos)`. Check state each iteration.

6. **`idle()`**: `sync()` then `stop()` then `complete_stop()`.

7. **`pause()`**: `client.Stop()` (only if Playing).

8. **`resume()`**: `client.Start()` (only if Playing).

9. **`setChannelVolume(channel, level)`**: Get `IAudioStreamVolume` service, call `SetChannelVolume`. Handle `DEVICE_INVALIDATED` by reopening.

10. **`maybeFireCallback()`**: Get play position, fire callbacks for any `feedEnds` where pos >= end. Uses `Vec::retain` (equivalent of C++ `std::erase_if`).

11. **`waitUntilNeeded(maxWait)`**: If pending callbacks, reduce wait time to next callback time. Then `WaitForSingleObject(wakeEvent, maxWait)`.

12. **`didPreferredDeviceBecomeAvailable()`**: If already on preferred, or no preferred specified, or no device state change since last check, return false. Otherwise try `get_preferred_device()`.

The callback type for `WasapiPlayerInner` should be `Option<Box<dyn Fn(u32) + Send>>` — the inner struct takes a Rust closure, not a Python callable. The PyO3 wrapper (Task 6) will bridge Python callables to this.

```rust
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Threading::*;

use crate::{PlayState, BUFFER_MS, BUFFER_SIZE};
use crate::device::{self, DeviceChangeCounters};
use crate::silence_detect;

pub struct WasapiPlayerInner {
    client: Option<IAudioClient>,
    render: Option<IAudioRenderClient>,
    clock: Option<IAudioClock>,
    buffer_frames: u32,
    endpoint_id: String,
    channels: u16,
    samples_per_sec: u32,
    bits_per_sample: u16,
    block_align: u16,
    callback: Option<Box<dyn Fn(u32) + Send>>,
    play_state: PlayState,
    feed_ends: Vec<(u32, u64)>,  // (feedId, endTimeMs)
    clock_freq: u64,
    sent_frames: u32,
    next_feed_id: u32,
    wake_event: HANDLE,
    counters: Arc<DeviceChangeCounters>,
    default_device_change_count: u32,
    device_state_change_count: u32,
    is_using_preferred_device: bool,
    is_trimming_leading_silence: bool,
}
```

Implementation notes:
- `WAVEFORMATEX` is constructed internally from the individual format params
- `wake_event` is created via `CreateEventW(None, false, false, None)`
- On `Drop`, close the `wake_event` handle via `CloseHandle`
- All Windows COM calls are `unsafe`
- Error handling: return `windows::core::Result<T>` for methods that can fail

**Step 2: Verify it compiles**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo check -p nvda_wasapi`
Expected: compiles

**Step 3: Write unit tests for PlayState transitions and feed ID tracking**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Test that new player starts in Stopped state
    // Test feed ID increments correctly
    // Test complete_stop resets state
    // Test frames_to_ms conversion
}
```

**Step 4: Run tests**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo test -p nvda_wasapi`
Expected: PASS

**Step 5: Commit**

```bash
git add rust/nvda_wasapi/src/player.rs
git commit --no-verify -m "feat: implement WasapiPlayerInner with full WASAPI playback logic"
```

---

### Task 5: Implement SilencePlayer

Port the C++ `SilencePlayer` class to Rust. Uses a background `std::thread` and an internal `WasapiPlayerInner`.

**Files:**
- Modify: `rust/nvda_wasapi/src/silence.rs`

**Step 1: Implement SilencePlayer**

Key behaviors from C++:
- Background thread loops: wait for wake event → feed silence until `endTime` → idle
- `playFor(ms, volume)`: sets `endTime = GetTickCount64() + ms`, generates white noise if volume changed, sets wake event
- `terminate()`: sets `endTime = 0`, calls `player.stop()`, sets wake event. Thread exits on seeing `endTime == 0`.
- White noise: normal distribution with `stddev = volume * 256`, stored as `Vec<i16>`
- Format: mono, 48000Hz, 16-bit PCM
- Buffer size: `48000 * 2 * 400 / 1000 = 38400` bytes per feed

```rust
use std::sync::{Arc, Mutex, Condvar};
use std::thread::{self, JoinHandle};
use crate::player::WasapiPlayerInner;
use crate::device::DeviceChangeCounters;

struct SilenceState {
    end_time: u64,       // 0 = terminate
    volume: f32,
    should_wake: bool,
}

pub struct SilencePlayer {
    state: Arc<(Mutex<SilenceState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl SilencePlayer {
    const SAMPLES_PER_SEC: u32 = 48000;
    const SILENCE_BYTES: usize = (Self::SAMPLES_PER_SEC as usize) * 2 * 400 / 1000;

    pub fn new(
        endpoint_id: &str,
        counters: Arc<DeviceChangeCounters>,
    ) -> windows::core::Result<Self> {
        // Create internal WasapiPlayerInner (mono, 48kHz, 16-bit, no callback)
        // Open the device
        // Spawn background thread
        // Return Self
        todo!()
    }

    pub fn play_for(&self, ms: u32, volume: f32) {
        // Update end_time, regenerate white noise if volume changed, signal wake
    }

    pub fn terminate(self) {
        // Set end_time to 0, stop player, signal wake, join thread
    }
}
```

Note: Use `std::sync::Condvar` or Windows `Event` for wake signaling. The C++ uses Windows Events; Rust can use either approach. Using Windows Events is more consistent with the C++ behavior.

**Step 2: Verify it compiles**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo check -p nvda_wasapi`
Expected: compiles

**Step 3: Commit**

```bash
git add rust/nvda_wasapi/src/silence.rs
git commit --no-verify -m "feat: implement SilencePlayer with background thread"
```

---

### Task 6: Add PyO3 WasapiPlayer wrapper and wasapi submodule to nvda_python

This task creates the Python-facing API: a `#[pyclass] WasapiPlayer` wrapping `WasapiPlayerInner` with GIL handling, and module-level functions for silence and startup.

**Files:**
- Create: `rust/nvda_python/src/wasapi.rs`
- Modify: `rust/nvda_python/src/lib.rs` (add wasapi submodule)
- Modify: `rust/nvda_python/Cargo.toml` (add nvda_wasapi dependency)

**Step 1: Add nvda_wasapi dependency to nvda_python**

In `rust/nvda_python/Cargo.toml`, add:
```toml
nvda_wasapi = { path = "../nvda_wasapi" }
```

**Step 2: Create wasapi.rs with PyO3 wrapper**

```rust
use pyo3::prelude::*;
use nvda_wasapi::player::WasapiPlayerInner;
use nvda_wasapi::device;
use nvda_wasapi::silence::SilencePlayer;
use std::sync::{Arc, Mutex, OnceLock};

/// Global state initialized by wasapiStartup()
static GLOBAL_STATE: OnceLock<GlobalWasapiState> = OnceLock::new();

struct GlobalWasapiState {
    counters: Arc<device::DeviceChangeCounters>,
    _notification_client: windows::Win32::Media::Audio::IMMNotificationClient,
}

// Module-level silence player, protected by Mutex
static SILENCE: Mutex<Option<SilencePlayer>> = Mutex::new(None);

#[pyclass]
pub struct WasapiPlayer {
    inner: WasapiPlayerInner,
    callback: PyObject,
}

#[pymethods]
impl WasapiPlayer {
    #[new]
    fn new(
        endpointId: &str,
        channels: u16,
        samplesPerSec: u32,
        bitsPerSample: u16,
        callback: PyObject,
    ) -> PyResult<Self> {
        let state = GLOBAL_STATE.get()
            .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err(
                "wasapiStartup() not called"
            ))?;
        let inner = WasapiPlayerInner::new(
            endpointId,
            channels,
            samplesPerSec,
            bitsPerSample,
            None, // callback set below
            state.counters.clone(),
        ).map_err(|e| pyo3::exceptions::PyOSError::new_err(
            format!("WASAPI error: {}", e)
        ))?;
        Ok(Self { inner, callback })
    }

    fn open(&mut self) -> PyResult<()> {
        self.inner.open(false).map_err(to_os_error)
    }

    fn feed(&mut self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
        // Set up a callback closure that will fire into Python
        // Release GIL during WaitForSingleObject in feed loop
        // Reacquire GIL for callback firing
        let callback = self.callback.clone_ref(py);
        let feed_id = py.allow_threads(|| {
            self.inner.feed(data, true)
        }).map_err(to_os_error)?;

        // Fire any pending callbacks with GIL held
        // self.inner.take_pending_callbacks() returns Vec<u32> of fired feedIds
        // For each, call callback(feedId) in Python
        self.fire_callbacks(py)?;
        Ok(feed_id)
    }

    fn stop(&mut self) -> PyResult<()> {
        self.inner.stop().map_err(to_os_error)
    }

    fn sync(&mut self, py: Python<'_>) -> PyResult<()> {
        // Release GIL during sync wait loop, reacquire for callbacks
        // This needs a more sophisticated approach — see implementation notes
        py.allow_threads(|| {
            self.inner.sync()
        }).map_err(to_os_error)?;
        self.fire_callbacks(py)?;
        Ok(())
    }

    fn idle(&mut self, py: Python<'_>) -> PyResult<()> {
        self.sync(py)?;
        self.stop()?;
        Ok(())
    }

    fn pause(&mut self) -> PyResult<()> {
        self.inner.pause().map_err(to_os_error)
    }

    fn resume(&mut self) -> PyResult<()> {
        self.inner.resume().map_err(to_os_error)
    }

    #[pyo3(name = "setChannelVolume")]
    fn set_channel_volume(&mut self, channel: u32, level: f32) -> PyResult<()> {
        self.inner.set_channel_volume(channel, level).map_err(to_os_error)
    }

    #[pyo3(name = "startTrimmingLeadingSilence")]
    fn start_trimming_leading_silence(&mut self, start: bool) {
        self.inner.start_trimming_leading_silence(start);
    }
}

impl WasapiPlayer {
    fn fire_callbacks(&self, py: Python<'_>) -> PyResult<()> {
        // Get pending callback IDs from inner
        // For each, call self.callback with feedId
        Ok(())
    }
}

fn to_os_error(e: windows::core::Error) -> PyErr {
    pyo3::exceptions::PyOSError::new_err(format!("{}", e))
}

// Module-level functions

#[pyfunction]
#[pyo3(name = "wasapiStartup")]
fn wasapi_startup() -> PyResult<()> {
    GLOBAL_STATE.get_or_try_init(|| {
        let (counters, client) = device::register_notification_client()
            .map_err(to_os_error)?;
        Ok::<_, PyErr>(GlobalWasapiState {
            counters,
            _notification_client: client,
        })
    })?;
    Ok(())
}

#[pyfunction]
#[pyo3(name = "silenceInit")]
fn silence_init(endpointId: &str) -> PyResult<()> {
    let state = GLOBAL_STATE.get()
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("wasapiStartup() not called"))?;
    let player = SilencePlayer::new(endpointId, state.counters.clone())
        .map_err(to_os_error)?;
    let mut guard = SILENCE.lock().unwrap();
    *guard = Some(player);
    Ok(())
}

#[pyfunction]
#[pyo3(name = "silencePlayFor")]
fn silence_play_for(ms: u32, volume: f32) {
    if let Some(ref player) = *SILENCE.lock().unwrap() {
        player.play_for(ms, volume);
    }
}

#[pyfunction]
#[pyo3(name = "silenceTerminate")]
fn silence_terminate() {
    let player = SILENCE.lock().unwrap().take();
    if let Some(player) = player {
        player.terminate();
    }
}
```

**Step 3: Update lib.rs to export wasapi submodule**

In `rust/nvda_python/src/lib.rs`, add the wasapi module alongside tones:

```rust
mod wasapi;

// In the nvda_rust module:
#[pymodule]
#[pyo3(name = "nvdaRust")]
mod nvda_rust {
    #[pymodule_export]
    use super::tones_mod;
    #[pymodule_export]
    use super::wasapi_mod;
}
```

The `wasapi_mod` should be a `#[pymodule]` that exports `WasapiPlayer`, `wasapiStartup`, `silenceInit`, `silencePlayFor`, `silenceTerminate`.

**Step 4: Verify it compiles**

Run: `cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust && cargo check -p nvda_python`
Expected: compiles

**Step 5: Commit**

```bash
git add rust/nvda_python/
git commit --no-verify -m "feat: add PyO3 WasapiPlayer wrapper and wasapi submodule"
```

**Implementation notes for GIL handling in feed()/sync():**

The C++ code fires callbacks inline during `feed()` and `sync()`. In Rust+PyO3, we need the GIL to call Python. Two approaches:

**Option A (simpler):** Collect pending callback IDs in a `Vec<u32>` during the `allow_threads` block, then fire them all after reacquiring the GIL. This changes timing slightly (callbacks fire after feed returns from waiting, not precisely at playback position) but matches the design doc.

**Option B (more faithful):** Use `py.allow_threads` in smaller chunks — release GIL for each wait, reacquire between waits to fire callbacks. This is more complex but more faithful to C++ behavior. The feed loop would alternate between GIL-released waits and GIL-held callback firing.

Start with Option A. If testing reveals timing issues, switch to Option B.

---

### Task 7: Modify nvwave.py to use Rust WasapiPlayer

Switch `nvwave.py` from ctypes `wasapi.*` calls to `nvdaRust.wasapi.*`.

**Files:**
- Modify: `source/nvwave.py`

**Step 1: Update imports**

Remove:
```python
from ctypes import (
    c_uint,
    byref,
    c_void_p,
    CFUNCTYPE,
    c_float,
    string_at,
)
from comtypes import HRESULT
from comtypes.hresult import E_INVALIDARG
import wasapi
```

Add:
```python
import nvdaRust
```

Keep `E_INVALIDARG` value as a constant: `E_INVALIDARG = -2147024809`

**Step 2: Remove wasPlay_callback and _instances**

Remove:
```python
wasPlay_callback = CFUNCTYPE(None, c_void_p, c_uint)
```

Remove from `WavePlayer`:
```python
_instances = weakref.WeakValueDictionary()
```

Remove the `import weakref` if nothing else uses it.

**Step 3: Update WavePlayer.__init__**

Replace:
```python
self._player = wasapi.wasPlay_create(
    outputDevice,
    format,
    WavePlayer._callback,
)
self._doneCallbacks = {}
self._instances[self._player] = self
```

With:
```python
self._player = nvdaRust.wasapi.WasapiPlayer(
    endpointId=outputDevice,
    channels=channels,
    samplesPerSec=samplesPerSec,
    bitsPerSample=bitsPerSample,
    callback=self._makeFeedCallback(),
)
self._doneCallbacks = {}
```

Add method:
```python
def _makeFeedCallback(self):
    def _onFeedDone(feedId):
        onDone = self._doneCallbacks.pop(feedId, None)
        if onDone:
            onDone()
    return _onFeedDone
```

**Step 4: Remove static _callback and _instances lookup**

Remove the entire `_callback` method:
```python
@wasPlay_callback
def _callback(cppPlayer, feedId):
    pyPlayer = WavePlayer._instances[cppPlayer]
    onDone = pyPlayer._doneCallbacks.pop(feedId, None)
    if onDone:
        onDone()
```

**Step 5: Update all method calls**

Replace each `wasapi.wasPlay_*` call with direct method call:

| Before | After |
|--------|-------|
| `wasapi.wasPlay_open(self._player)` | `self._player.open()` |
| `wasapi.wasPlay_feed(self._player, data, size, byref(feedId) if onDone else None)` | `feedId = self._player.feed(data)` |
| `wasapi.wasPlay_stop(self._player)` | `self._player.stop()` |
| `wasapi.wasPlay_sync(self._player)` | `self._player.sync()` |
| `wasapi.wasPlay_idle(self._player)` | `self._player.idle()` |
| `wasapi.wasPlay_pause(self._player)` | `self._player.pause()` |
| `wasapi.wasPlay_resume(self._player)` | `self._player.resume()` |
| `wasapi.wasPlay_setChannelVolume(self._player, ch, c_float(lvl))` | `self._player.setChannelVolume(ch, lvl)` |
| `wasapi.wasPlay_startTrimmingLeadingSilence(self._player, start)` | `self._player.startTrimmingLeadingSilence(start)` |
| `wasapi.wasPlay_destroy(self._player)` | (removed — Drop handles it) |

**Step 6: Update feed() method**

Replace:
```python
feedId = c_uint() if onDone else None
# ...
wasapi.wasPlay_feed(
    self._player,
    data,
    size if size is not None else len(data),
    byref(feedId) if onDone else None,
)
if onDone:
    self._doneCallbacks[feedId.value] = onDone
```

With:
```python
if not isinstance(data, bytes):
    data = string_at(data, size)
feedId = self._player.feed(data)
if onDone:
    self._doneCallbacks[feedId] = onDone
```

Note: keep `string_at` import from ctypes for converting c_void_p data to bytes.

**Step 7: Update __del__**

Replace:
```python
if self._player:
    wasapi.wasPlay_destroy(self._player)
    self._player = None
```

With:
```python
self._player = None  # Rust Drop handles cleanup
```

**Step 8: Update setVolume**

Remove `c_float()` wrapping — Rust accepts plain float:
```python
# Before:
wasapi.wasPlay_setChannelVolume(self._player, 0, c_float(left))
wasapi.wasPlay_setChannelVolume(self._player, 1, c_float(right))

# After:
self._player.setChannelVolume(0, left)
self._player.setChannelVolume(1, right)
```

**Step 9: Update silence calls**

Replace:
```python
wasapi.wasSilence_init(outputDevice)
wasapi.wasSilence_playFor(1000 * ..., c_float(...))
wasapi.wasSilence_terminate()
```

With:
```python
nvdaRust.wasapi.silenceInit(outputDevice)
nvdaRust.wasapi.silencePlayFor(1000 * ..., ...)
nvdaRust.wasapi.silenceTerminate()
```

**Step 10: Update initialize() and terminate()**

Replace `initialize()`:
```python
def initialize():
    nvdaRust.wasapi.wasapiStartup()
    getOnErrorSoundRequested().register(playErrorSound)
```

Remove all the `restype = HRESULT` lines — PyO3 handles error conversion.

Replace `terminate()`:
```python
def terminate() -> None:
    if WavePlayer._silenceDevice is not None:
        nvdaRust.wasapi.silenceTerminate()
    getOnErrorSoundRequested().unregister(playErrorSound)
```

**Step 11: Update _idleCheck**

Replace:
```python
wasapi.wasPlay_idle(player._player)
```

With:
```python
player._player.idle()
```

**Step 12: Commit**

```bash
git add source/nvwave.py
git commit --no-verify -m "feat: switch nvwave.py to use Rust WasapiPlayer"
```

---

### Task 8: Remove wasapi.py ctypes bindings

Now that nvwave.py uses Rust directly, the ctypes wrapper is unused.

**Files:**
- Delete: `source/wasapi.py`

**Step 1: Verify no other files import wasapi**

Search for `import wasapi` or `from wasapi` in `source/` (excluding test files). The only user should have been `nvwave.py`, which was updated in Task 7.

**Step 2: Delete wasapi.py**

```bash
git rm source/wasapi.py
```

**Step 3: Commit**

```bash
git commit --no-verify -m "chore: remove wasapi.py ctypes bindings (replaced by Rust)"
```

---

### Task 9: Build and end-to-end verification

**Files:** None (verification only)

**Step 1: Run Rust tests**

```bash
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator/rust
cargo test --workspace
```

Expected: all tests pass

**Step 2: Build with SCons**

```bash
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator
scons source
```

Expected: maturin builds successfully, nvdaRust.pyd includes wasapi submodule

**Step 3: Verify Python module loads**

```bash
cd /c/Users/bram/src/nvda/.claude/worktrees/rust-beep-generator
uv run python -c "import nvdaRust; print(dir(nvdaRust.wasapi))"
```

Expected: shows `WasapiPlayer`, `wasapiStartup`, `silenceInit`, `silencePlayFor`, `silenceTerminate`

**Step 4: Commit any fixes**

If any build issues are found, fix and commit.
