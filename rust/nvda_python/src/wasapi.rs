#![allow(non_snake_case)]

use std::sync::{Arc, Mutex, OnceLock};

use pyo3::prelude::*;

use nvda_wasapi::device::{self, DeviceChangeCounters};
use nvda_wasapi::player::{DeviceControlHandle, WasapiPlayerInner};
use nvda_wasapi::silence::SilencePlayer;
use windows::Win32::Media::Audio::IMMNotificationClient;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct GlobalWasapiState {
    counters: Arc<DeviceChangeCounters>,
    _notification_client: IMMNotificationClient,
}

// SAFETY: IMMNotificationClient is a COM pointer registered with the system
// enumerator. It is ref-counted and thread-safe (MTA). We only read
// `counters` from it after construction, so sending/sharing across threads
// is safe.
unsafe impl Send for GlobalWasapiState {}
unsafe impl Sync for GlobalWasapiState {}

static GLOBAL_STATE: OnceLock<GlobalWasapiState> = OnceLock::new();

static SILENCE_PLAYER: Mutex<Option<SilencePlayer>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

fn to_os_error(e: windows::core::Error) -> PyErr {
    let code = e.code().0;
    pyo3::exceptions::PyOSError::new_err(format!(
        "WASAPI error: {} (0x{:08X})",
        e.message(),
        code as u32
    ))
}

// ---------------------------------------------------------------------------
// WasapiPlayer pyclass
// ---------------------------------------------------------------------------

/// A sendable wrapper around a raw pointer to WasapiPlayerInner.
/// Used to pass the pointer into `py.detach()` closures.
struct SendPtr(*mut WasapiPlayerInner);
// SAFETY: WasapiPlayerInner is Send, and we only use this pointer while
// the Mutex is held (the MutexGuard stays alive across detach).
unsafe impl Send for SendPtr {}

impl SendPtr {
    /// Get a mutable reference to the inner player.
    ///
    /// # Safety
    /// The caller must ensure exclusive access to the WasapiPlayerInner --
    /// the surrounding code holds the player Mutex via a `MutexGuard` that
    /// outlives the returned reference, so this is sound for our usage.
    /// The clippy lint is suppressed because this pattern is the standard
    /// idiom for handing a mutable view across a `py.detach()` closure
    /// boundary.
    #[allow(clippy::mut_from_ref)]
    unsafe fn as_mut(&self) -> &mut WasapiPlayerInner {
        &mut *self.0
    }
}

/// WasapiPlayer wraps WasapiPlayerInner for Python.
///
/// The inner player is behind a Mutex so that `&self` methods can be used
/// (allowing concurrent Python threads). The `control_handle` provides a
/// separate, lock-free path to stop/pause/resume playback from another thread
/// while `feed()` is blocking with the mutex held.
///
/// For blocking methods (`feed`, `sync`, `idle`) — see `run_blocking` — we:
/// 1. Lock the mutex
/// 2. Get a raw pointer to the inner player
/// 3. Release the GIL via `py.detach()` while keeping the mutex locked
/// 4. The blocking call runs without the GIL
/// 5. On return, we re-acquire the GIL and fire callbacks
///
/// For `stop`/`pause`/`resume`, we use the `control_handle`, which bypasses the
/// mutex entirely (using only atomic state and thread-safe Windows APIs) — so
/// they work even while a blocking `feed()` holds the mutex. This matters most
/// for pause/resume: a paused stream keeps `feed()` blocked in its backpressure
/// wait, so routing them through the mutex would deadlock.
///
/// The `control_handle` is stable across device reopens -- it references the
/// same atomic play_state and wake_event that persist for the lifetime
/// of the inner player, so no Mutex is needed around it.
#[pyclass]
pub struct WasapiPlayer {
    inner: Mutex<WasapiPlayerInner>,
    control_handle: DeviceControlHandle,
    /// The Python feed-done callable, invoked by [`WasapiPlayer::fire_pending`]
    /// once per completed feed id.
    callback: Py<PyAny>,
    /// Completed feed ids awaiting delivery to `callback`. The inner-crate
    /// callback closure (which runs while the `inner` mutex is held) only
    /// pushes ids here — a cheap Rust-only lock, never the GIL. The Python
    /// callback is fired later by `fire_pending`, AFTER `inner` is released,
    /// so `feed`/`sync` never hold `inner` while re-acquiring the GIL. That
    /// ordering (inner-then-GIL under the lock, GIL-then-inner in every other
    /// method) is what deadlocked against `setChannelVolume` etc.
    pending: Arc<Mutex<Vec<u32>>>,
}

impl WasapiPlayer {
    /// Drain the completed-feed-id queue and invoke the Python callback for
    /// each, in order. Must be called with the GIL held and, crucially,
    /// WITHOUT holding `inner` — the callback runs arbitrary Python that may
    /// re-enter player methods. Callback errors are logged and swallowed,
    /// matching the historical WINFUNCTYPE behavior.
    fn fire_pending(&self, py: Python<'_>) {
        // Only the pending queue's own lock is held here, and only long
        // enough to move the ids out; the callbacks fire with no lock held.
        let ids: Vec<u32> = {
            let mut q = self.pending.lock().unwrap();
            std::mem::take(&mut *q)
        };
        for id in ids {
            if let Err(e) = self.callback.call1(py, (id,)) {
                log::warn!(
                    "WasapiPlayer feed-done callback raised: {e:?} (feed_id={id})",
                );
            }
        }
    }

    /// Run a blocking inner-player call (feed/sync/idle) with the GIL released
    /// but the inner mutex held for the whole call, then deliver any feed-done
    /// callbacks it queued. The MutexGuard is created here and outlives
    /// `py.detach()`; the raw pointer handed across the boundary via `SendPtr`
    /// is sound only because of that (see `SendPtr::as_mut`). Only `stop`/
    /// `pause`/`resume` (via the control handle) can touch the device while
    /// this holds the mutex.
    fn run_blocking<R: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut WasapiPlayerInner) -> windows::core::Result<R> + Send,
    ) -> PyResult<R> {
        let result = {
            let mut player = self.inner.lock().unwrap();
            let player_ptr = SendPtr(&mut *player as *mut WasapiPlayerInner);
            py.detach(move || f(unsafe { player_ptr.as_mut() }))
        };
        // `inner` is released; deliver callbacks the call queued (see
        // `pending` / `fire_pending`), never while holding the mutex.
        self.fire_pending(py);
        result.map_err(to_os_error)
    }

    /// Run a quick inner-player call (open/setChannelVolume) that queues no
    /// callbacks. The GIL is released *before* the inner mutex is acquired
    /// (GIL-before-lock ordering), preventing a deadlock with `feed()`, which
    /// holds the mutex and waits for the GIL.
    fn with_inner<R: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut WasapiPlayerInner) -> windows::core::Result<R> + Send,
    ) -> PyResult<R> {
        let inner = &self.inner;
        py.detach(move || f(&mut inner.lock().unwrap())).map_err(to_os_error)
    }
}

#[pymethods]
impl WasapiPlayer {
    #[new]
    #[pyo3(signature = (endpointId, channels, samplesPerSec, bitsPerSample, callback))]
    fn new(
        endpointId: &str,
        channels: u16,
        samplesPerSec: u32,
        bitsPerSample: u16,
        callback: Py<PyAny>,
    ) -> PyResult<Self> {
        let global = GLOBAL_STATE.get().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "wasapiStartup() must be called before creating a WasapiPlayer",
            )
        })?;
        let counters = global.counters.clone();

        // The inner player invokes this closure while it holds the `inner`
        // mutex (from inside the WASAPI feed loop). It must therefore NOT
        // acquire the GIL — doing so established an inner-then-GIL lock order
        // that deadlocked against every other method's GIL-then-inner order
        // (e.g. setChannelVolume). Instead it just records the completed feed
        // id in a Rust-only queue; the Python callback is fired afterwards by
        // `fire_pending`, once `inner` has been released. onDone latency is
        // still low: the queue is drained at the end of the same feed()/sync()
        // call, not on the ~100ms feed-loop timer the old queue-and-drain used.
        let pending: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_for_inner = Arc::clone(&pending);
        let callback_fn: Box<dyn Fn(u32) + Send> = Box::new(move |feed_id: u32| {
            pending_for_inner.lock().unwrap().push(feed_id);
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

        let control_handle = inner.control_handle();

        Ok(Self {
            inner: Mutex::new(inner),
            control_handle,
            callback,
            pending,
        })
    }

    fn open(&self, py: Python<'_>) -> PyResult<()> {
        self.with_inner(py, |player| player.open(false))
    }

    fn feed(&self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
        // Copy the audio out of the Python buffer before releasing the GIL:
        // the feed runs without the GIL held, so it must not read a buffer
        // Python could free or mutate underneath it.
        let data_owned = data.to_vec();
        self.run_blocking(py, move |player| player.feed(&data_owned, true))
    }

    fn stop(&self) -> PyResult<()> {
        // The DeviceControlHandle is always safe to call from any thread. It calls
        // IAudioClient::Stop() directly via the shared client slot, so the
        // device halts immediately even when feed() is currently holding
        // the player mutex. (Previously this fell back to a signal-only
        // path when feed() held the mutex, which let up to ~BUFFER_MS of
        // already-queued audio keep playing.)
        self.control_handle.stop();
        Ok(())
    }

    fn sync(&self, py: Python<'_>) -> PyResult<()> {
        self.run_blocking(py, |player| player.sync())
    }

    fn idle(&self, py: Python<'_>) -> PyResult<()> {
        self.run_blocking(py, |player| player.idle())
    }

    fn pause(&self, py: Python<'_>) -> PyResult<()> {
        // Route through the lock-free DeviceControlHandle rather than locking inner:
        // pause() is called while feed() is blocked in its backpressure wait
        // (still holding inner) and the whole point of pausing is that the
        // device is not draining, so feed() will not release inner until we
        // resume. Locking inner here would therefore deadlock. The handle
        // calls IAudioClient::Stop() directly via the shared client slot,
        // leaving play_state as Playing so resume() can continue. GIL is
        // released around the COM call for consistency with the other methods.
        py.detach(|| self.control_handle.pause()).map_err(to_os_error)
    }

    fn resume(&self, py: Python<'_>) -> PyResult<()> {
        // See pause(): route through the lock-free DeviceControlHandle. resume() calling
        // IAudioClient::Start() is what unblocks a feed() stuck in its
        // backpressure wait, so it must not itself require the inner mutex.
        py.detach(|| self.control_handle.resume()).map_err(to_os_error)
    }

    #[pyo3(name = "setChannelVolume")]
    fn set_channel_volume(&self, py: Python<'_>, channel: u32, level: f32) -> PyResult<()> {
        self.with_inner(py, move |player| player.set_channel_volume(channel, level))
    }

    #[pyo3(name = "startTrimmingLeadingSilence")]
    fn start_trimming_leading_silence(&self, py: Python<'_>, start: bool) {
        // Release the GIL before locking inner to prevent deadlock with feed().
        let inner = &self.inner;
        py.detach(move || {
            let mut player = inner.lock().unwrap();
            player.start_trimming_leading_silence(start);
        });
    }
}

// ---------------------------------------------------------------------------
// Module-level functions
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(name = "wasapiStartup")]
pub fn wasapi_startup() -> PyResult<()> {
    if GLOBAL_STATE.get().is_some() {
        return Ok(());
    }
    let (counters, notification_client) =
        device::register_notification_client().map_err(to_os_error)?;
    let _ = GLOBAL_STATE.set(GlobalWasapiState {
        counters,
        _notification_client: notification_client,
    });
    Ok(())
}

#[pyfunction]
#[pyo3(name = "silenceInit", signature = (endpointId))]
pub fn silence_init(endpointId: &str) -> PyResult<()> {
    let global = GLOBAL_STATE.get().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "wasapiStartup() must be called before silenceInit()",
        )
    })?;
    let counters = global.counters.clone();

    let mut player = SilencePlayer::new(endpointId, counters).map_err(to_os_error)?;
    player.init().map_err(to_os_error)?;

    let mut guard = SILENCE_PLAYER.lock().unwrap();
    // Terminate any existing player first.
    if let Some(mut old) = guard.take() {
        old.terminate();
    }
    *guard = Some(player);
    Ok(())
}

#[pyfunction]
#[pyo3(name = "silencePlayFor")]
pub fn silence_play_for(ms: u32, volume: f32) -> PyResult<()> {
    let guard = SILENCE_PLAYER.lock().unwrap();
    let player = guard.as_ref().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "silenceInit() must be called before silencePlayFor()",
        )
    })?;
    player.play_for(ms, volume);
    Ok(())
}

#[pyfunction]
#[pyo3(name = "silenceTerminate")]
pub fn silence_terminate() -> PyResult<()> {
    let mut guard = SILENCE_PLAYER.lock().unwrap();
    if let Some(mut player) = guard.take() {
        player.terminate();
    }
    Ok(())
}

/// Check whether any audio render device is currently playing audio.
///
/// Returns True if audio is detected or on error (conservative).
/// This is the Rust replacement for the C++ `audioDucking_shouldDelay()`.
#[pyfunction]
#[pyo3(name = "audioDucking_shouldDelay")]
pub fn audio_ducking_should_delay() -> bool {
    device::is_any_audio_playing()
}
