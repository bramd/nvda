#![allow(non_snake_case)]

use std::sync::{Arc, Mutex, OnceLock};

use pyo3::prelude::*;

use nvda_wasapi::device::{self, DeviceChangeCounters};
use nvda_wasapi::player::{StopHandle, WasapiPlayerInner};
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
    /// The caller must ensure exclusive access to the WasapiPlayerInner.
    unsafe fn as_mut(&self) -> &mut WasapiPlayerInner {
        &mut *self.0
    }
}

/// WasapiPlayer wraps WasapiPlayerInner for Python.
///
/// The inner player is behind a Mutex so that `&self` methods can be used
/// (allowing concurrent Python threads). The `stop_handle` provides a
/// separate, lock-free path to stop playback from another thread while
/// `feed()` is blocking with the mutex held.
///
/// For blocking methods (`feed`, `sync`, `idle`), we:
/// 1. Lock the mutex
/// 2. Get a raw pointer to the inner player
/// 3. Release the GIL via `py.detach()` while keeping the mutex locked
/// 4. The blocking call runs without the GIL
/// 5. On return, we re-acquire the GIL and fire callbacks
///
/// For `stop()`, we use the StopHandle which bypasses the mutex entirely,
/// using only atomic state and thread-safe Windows APIs.
#[pyclass]
pub struct WasapiPlayer {
    inner: Mutex<WasapiPlayerInner>,
    stop_handle: Mutex<StopHandle>,
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
            stop_handle: Mutex::new(stop_handle),
            callback,
            pending_callbacks,
        })
    }

    fn open(&self) -> PyResult<()> {
        let mut player = self.inner.lock().unwrap();
        player.open(false).map_err(to_os_error)?;
        // Replace the stop handle with a fresh one that references the
        // (possibly new) audio client.
        let mut sh = self.stop_handle.lock().unwrap();
        *sh = player.stop_handle();
        Ok(())
    }

    fn feed(&self, py: Python<'_>, data: &[u8]) -> PyResult<u32> {
        let data_owned = data.to_vec();
        let feed_id;
        {
            let mut player = self.inner.lock().unwrap();
            let player_ptr = SendPtr(&mut *player as *mut WasapiPlayerInner);
            // Release the GIL while feeding. The mutex remains locked,
            // preventing other methods (except stop via StopHandle) from
            // accessing inner. This matches the C++ behavior where the GIL
            // is released for all ctypes calls.
            feed_id = py.detach(move || unsafe {
                player_ptr.as_mut().feed(Some(&data_owned), true)
            }).map_err(to_os_error)?;
            // MutexGuard drops here, releasing the mutex.
        }
        self.fire_pending_callbacks(py)?;
        Ok(feed_id)
    }

    fn stop(&self) -> PyResult<()> {
        // Use the StopHandle to stop without acquiring the inner mutex.
        // This allows stop() to interrupt a blocking feed() call.
        let sh = self.stop_handle.lock().unwrap();
        sh.stop().map_err(to_os_error)
    }

    fn sync(&self, py: Python<'_>) -> PyResult<()> {
        {
            let mut player = self.inner.lock().unwrap();
            let player_ptr = SendPtr(&mut *player as *mut WasapiPlayerInner);
            py.detach(move || unsafe {
                player_ptr.as_mut().sync()
            }).map_err(to_os_error)?;
        }
        self.fire_pending_callbacks(py)?;
        Ok(())
    }

    fn idle(&self, py: Python<'_>) -> PyResult<()> {
        {
            let mut player = self.inner.lock().unwrap();
            let player_ptr = SendPtr(&mut *player as *mut WasapiPlayerInner);
            py.detach(move || unsafe {
                player_ptr.as_mut().idle()
            }).map_err(to_os_error)?;
        }
        self.fire_pending_callbacks(py)?;
        Ok(())
    }

    fn pause(&self) -> PyResult<()> {
        let mut player = self.inner.lock().unwrap();
        player.pause().map_err(to_os_error)
    }

    fn resume(&self) -> PyResult<()> {
        let mut player = self.inner.lock().unwrap();
        player.resume().map_err(to_os_error)
    }

    #[pyo3(name = "setChannelVolume")]
    fn set_channel_volume(&self, channel: u32, level: f32) -> PyResult<()> {
        let mut player = self.inner.lock().unwrap();
        player.set_channel_volume(channel, level).map_err(to_os_error)
    }

    #[pyo3(name = "startTrimmingLeadingSilence")]
    fn start_trimming_leading_silence(&self, start: bool) {
        let mut player = self.inner.lock().unwrap();
        player.start_trimming_leading_silence(start);
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
