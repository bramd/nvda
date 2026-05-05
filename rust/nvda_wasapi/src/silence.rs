use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForSingleObject, INFINITE,
};

use crate::device::DeviceChangeCounters;
use crate::player::{StopHandle, WasapiPlayerInner};
use crate::BUFFER_MS;

/// Sample rate for silence/white-noise playback (mono 16-bit PCM).
const SILENCE_SAMPLES_PER_SEC: u32 = 48000;

/// Number of bytes to feed per iteration, matching the C++ constant:
/// `SAMPLES_PER_SEC * 2 * BUFFER_MS / 1000`
const SILENCE_BYTES: usize =
    (SILENCE_SAMPLES_PER_SEC as usize) * 2 * (BUFFER_MS as usize) / 1000;

/// Number of i16 samples per feed buffer.
const SILENCE_SAMPLES: usize = SILENCE_BYTES / 2;

/// Wrapper to make HANDLE Send-safe. HANDLE is just an opaque pointer-sized
/// value and Windows events are safe to signal/wait from any thread.
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

/// Shared mutable state between the owner thread and the background silence
/// thread.
struct SharedState {
    /// Tick count (ms) at which silence should stop. 0 means "terminate".
    end_time: u64,
    /// Current volume level. Negative means "not yet set".
    volume: f32,
    /// Pre-generated white-noise samples reused across feeds.
    white_noise: Vec<i16>,
}

/// Plays silence (or low-level white noise) in the background so the audio
/// device stays active and doesn't introduce latency spikes when real audio
/// starts.
pub struct SilencePlayer {
    state: Arc<Mutex<SharedState>>,
    player: Arc<Mutex<WasapiPlayerInner>>,
    /// Lock-free handle to interrupt a blocking `feed()` on the background
    /// thread without acquiring the player mutex.
    stop_handle: StopHandle,
    wake_event: HANDLE,
    thread: Option<JoinHandle<()>>,
}

// SAFETY: SilencePlayer is safe to send between threads. The HANDLE is an
// opaque kernel object handle that is thread-safe, and the player is behind
// Arc<Mutex<>>.
unsafe impl Send for SilencePlayer {}

impl SilencePlayer {
    /// Create a new `SilencePlayer` targeting the given endpoint (pass empty
    /// string for the default device).
    pub fn new(
        endpoint_id: &str,
        counters: Arc<DeviceChangeCounters>,
    ) -> windows::core::Result<Self> {
        let player = WasapiPlayerInner::new(
            endpoint_id,
            1,     // mono
            SILENCE_SAMPLES_PER_SEC,
            16,    // bits per sample
            None,  // no callback
            counters,
        )?;

        let wake_event = unsafe { CreateEventW(None, false, false, None)? };

        let stop_handle = player.stop_handle();

        let state = Arc::new(Mutex::new(SharedState {
            end_time: 0,
            volume: -1.0,
            white_noise: vec![0i16; SILENCE_SAMPLES],
        }));

        Ok(Self {
            state,
            player: Arc::new(Mutex::new(player)),
            stop_handle,
            wake_event,
            thread: None,
        })
    }

    /// Open the audio device and start the background thread.
    pub fn init(&mut self) -> windows::core::Result<()> {
        {
            let mut player = self.player.lock().unwrap();
            player.open(false)?;
        }

        let state = Arc::clone(&self.state);
        let player = Arc::clone(&self.player);
        let wake_event = SendHandle(self.wake_event);

        let handle = thread::spawn(move || {
            Self::run(state, player, wake_event);
        });
        self.thread = Some(handle);
        Ok(())
    }

    /// Request silence (or white noise at the given volume) for `ms`
    /// milliseconds. Pass `u32::MAX` for indefinite playback.
    pub fn play_for(&self, ms: u32, volume: f32) {
        let mut state = self.state.lock().unwrap();
        if volume != state.volume {
            generate_white_noise(&mut state.white_noise, volume);
            state.volume = volume;
        }
        state.end_time = if ms == u32::MAX {
            u64::MAX
        } else {
            tick_count_64() + ms as u64
        };
        drop(state);
        unsafe {
            let _ = SetEvent(self.wake_event);
        }
    }

    /// Stop playback and ask the background thread to exit.
    pub fn terminate(&mut self) {
        {
            let mut state = self.state.lock().unwrap();
            state.end_time = 0;
        }
        // Interrupt any ongoing feed using the lock-free StopHandle.
        // We must NOT lock self.player here -- the background thread may be
        // holding the player lock inside feed(), which would deadlock.
        self.stop_handle.stop();
        // Wake the thread if it is waiting on the event.
        unsafe {
            let _ = SetEvent(self.wake_event);
        }
        // Wait for the background thread to finish.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    // ---- background thread entry point ----

    fn run(
        state: Arc<Mutex<SharedState>>,
        player: Arc<Mutex<WasapiPlayerInner>>,
        wake_event: SendHandle,
    ) {
        let event = wake_event.0;
        loop {
            // Wait for a wake signal.
            unsafe {
                WaitForSingleObject(event, INFINITE);
            }

            // Check whether we should terminate.
            let end_time = {
                let s = state.lock().unwrap();
                s.end_time
            };
            if end_time == 0 {
                return;
            }

            // Feed silence/noise until the requested end time.
            loop {
                // Read the current end_time (may be updated by playFor or
                // terminate while we are looping).
                let end_time = {
                    let s = state.lock().unwrap();
                    s.end_time
                };
                if end_time == 0 || tick_count_64() >= end_time {
                    break;
                }

                // Pump silence/white-noise into the player. When volume is
                // zero we use feed_silence(), which routes through
                // AUDCLNT_BUFFERFLAGS_SILENT and avoids both the per-cycle
                // memcpy and the Vec allocation. With non-zero volume we
                // need to actually feed the white-noise samples.
                let volume_zero = {
                    let s = state.lock().unwrap();
                    s.volume == 0.0
                };
                let mut p = player.lock().unwrap();
                if volume_zero {
                    let _ = p.feed_silence(SILENCE_SAMPLES as u32);
                } else {
                    // Copy the noise outside the feed() call so we don't
                    // hold the state lock across the COM call. (state and
                    // player are independent locks; this matches the
                    // pattern used elsewhere.)
                    drop(p);
                    let feed_data: Vec<u8> = {
                        let s = state.lock().unwrap();
                        let bytes: &[u8] = unsafe {
                            std::slice::from_raw_parts(
                                s.white_noise.as_ptr() as *const u8,
                                s.white_noise.len() * 2,
                            )
                        };
                        bytes.to_vec()
                    };
                    let mut p = player.lock().unwrap();
                    let _ = p.feed(&feed_data, false);
                }
            }

            // Check if we were asked to terminate while feeding.
            let end_time = {
                let s = state.lock().unwrap();
                s.end_time
            };
            if end_time == 0 {
                return;
            }

            // Done playing -- idle the player.
            let mut p = player.lock().unwrap();
            let _ = p.idle();
        }
    }
}

impl Drop for SilencePlayer {
    fn drop(&mut self) {
        // Make sure the background thread is stopped.
        if self.thread.is_some() {
            self.terminate();
        }
        if !self.wake_event.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.wake_event);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the current tick count in milliseconds (wraps `GetTickCount64`).
fn tick_count_64() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

/// Fill `buf` with normally-distributed white noise at the given volume.
/// Uses a simple xorshift64 PRNG + Box-Muller transform to avoid pulling
/// in the `rand` crate.
fn generate_white_noise(buf: &mut [i16], volume: f32) {
    if volume == 0.0 {
        for s in buf.iter_mut() {
            *s = 0;
        }
        return;
    }

    let stddev = volume as f64 * 256.0;
    let mut rng_state: u64 = 0xDEAD_BEEF_CAFE_BABE;

    // Process pairs of samples using Box-Muller.
    let mut i = 0;
    while i + 1 < buf.len() {
        let (n1, n2) = box_muller(&mut rng_state, stddev);
        buf[i] = clamp_i16(n1);
        buf[i + 1] = clamp_i16(n2);
        i += 2;
    }
    // Handle a possible trailing sample.
    if i < buf.len() {
        let (n1, _) = box_muller(&mut rng_state, stddev);
        buf[i] = clamp_i16(n1);
    }
}

/// Xorshift64 PRNG -- returns a pseudo-random u64.
#[inline]
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Return a uniform random f64 in (0, 1).
#[inline]
fn uniform_01(state: &mut u64) -> f64 {
    // Use 53 bits of the xorshift output for a double in (0,1).
    let bits = xorshift64(state) >> 11;
    // Add 0.5 to avoid exact 0 (log(0) is -inf).
    (bits as f64 + 0.5) / ((1u64 << 53) as f64)
}

/// Box-Muller transform: produce two independent normal samples.
#[inline]
fn box_muller(state: &mut u64, stddev: f64) -> (f64, f64) {
    let u1 = uniform_01(state);
    let u2 = uniform_01(state);
    let mag = stddev * (-2.0 * u1.ln()).sqrt();
    let angle = 2.0 * std::f64::consts::PI * u2;
    (mag * angle.cos(), mag * angle.sin())
}

/// Clamp an f64 to i16 range.
#[inline]
fn clamp_i16(v: f64) -> i16 {
    v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}
