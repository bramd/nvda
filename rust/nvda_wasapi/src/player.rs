use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::{
    IAudioClient, IAudioClock, IAudioRenderClient, IAudioStreamVolume,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::device::{
    self, DeviceChangeCounters,
};
use crate::silence_detect;
use crate::{PlayState, BUFFER_SIZE};

/// AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM (0x80000000)
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x80000000;
/// AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY (0x08000000)
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x08000000;

/// AUDCLNT_E_DEVICE_INVALIDATED
const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x88890004u32 as i32;
/// AUDCLNT_E_NOT_INITIALIZED
const AUDCLNT_E_NOT_INITIALIZED: i32 = 0x88890001u32 as i32;

/// WAVE_FORMAT_PCM
const WAVE_FORMAT_PCM: u16 = 1;

/// Check whether an HRESULT matches a specific error code.
fn is_error(hr: &windows::core::Error, code: i32) -> bool {
    hr.code().0 == code
}

/// Source of frames for [`WasapiPlayerInner::feed_inner`]: real audio
/// data (`Data`) or `n` frames of silence emitted via
/// `AUDCLNT_BUFFERFLAGS_SILENT` (`Silence`).
enum FeedSource<'a> {
    Data(&'a [u8]),
    Silence(u32),
}

/// Thread-safe shared cell holding the current `IAudioClient`.
///
/// The same `Arc<ClientSlot>` is held by `WasapiPlayerInner` and any
/// [`DeviceControlHandle`] it creates, so a stop/pause/resume from another
/// thread can call `IAudioClient::Stop()`/`Start()` directly on the current
/// client (halting or restarting audio immediately) without acquiring the
/// player mutex. The slot is updated atomically by `open()` when the device is
/// reopened, so a control call racing against a device-change reopen acts on
/// whichever client is current at the moment the slot is read; driving a
/// just-replaced client is harmless.
pub(crate) struct ClientSlot {
    inner: Mutex<Option<IAudioClient>>,
}

// SAFETY: `IAudioClient` methods are documented as thread-safe by Microsoft
// (the COM apartment model places audio session objects in the MTA). The
// `Mutex` provides synchronisation for the swap operation. We declare Send
// + Sync explicitly because windows-rs interface types do not auto-derive
// these traits.
unsafe impl Send for ClientSlot {}
unsafe impl Sync for ClientSlot {}

impl ClientSlot {
    fn new() -> Self {
        Self { inner: Mutex::new(None) }
    }

    /// Snapshot the current client (cloning AddRef's the COM pointer).
    /// The returned Option is independent of subsequent slot updates.
    fn snapshot(&self) -> Option<IAudioClient> {
        self.inner.lock().unwrap().clone()
    }

    fn replace(&self, new_client: Option<IAudioClient>) {
        *self.inner.lock().unwrap() = new_client;
    }

    fn is_some(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }
}

/// Core WASAPI audio player. This is the internal struct without any PyO3
/// bindings -- it will be wrapped by a Python-facing class later.
pub struct WasapiPlayerInner {
    client_slot: Arc<ClientSlot>,
    render: Option<IAudioRenderClient>,
    clock: Option<IAudioClock>,
    buffer_frames: u32,
    endpoint_id: String,
    channels: u16,
    samples_per_sec: u32,
    bits_per_sample: u16,
    block_align: u16,
    callback: Option<Box<dyn Fn(u32) + Send>>,
    play_state: Arc<AtomicU8>,
    /// Maps feed ids to the end of their audio in ms since the start of the
    /// stream. Used to fire the callback at the right time.
    feed_ends: Vec<(u32, u64)>,
    clock_freq: u64,
    /// Total number of frames buffered so far.
    sent_frames: u32,
    next_feed_id: u32,
    wake_event: HANDLE,
    counters: Arc<DeviceChangeCounters>,
    default_device_change_count: u32,
    device_state_change_count: u32,
    is_using_preferred_device: bool,
    is_trimming_leading_silence: bool,
}

impl WasapiPlayerInner {
    /// Create a new WasapiPlayerInner.
    ///
    /// `endpoint_id`: pass an empty string to use the default device.
    /// `callback`: called with a feed id when that chunk finishes playing.
    pub fn new(
        endpoint_id: &str,
        channels: u16,
        samples_per_sec: u32,
        bits_per_sample: u16,
        callback: Option<Box<dyn Fn(u32) + Send>>,
        counters: Arc<DeviceChangeCounters>,
    ) -> windows::core::Result<Self> {
        let wake_event = unsafe { CreateEventW(None, false, false, None)? };
        let block_align = channels * bits_per_sample / 8;
        Ok(Self {
            client_slot: Arc::new(ClientSlot::new()),
            render: None,
            clock: None,
            buffer_frames: 0,
            endpoint_id: endpoint_id.to_string(),
            channels,
            samples_per_sec,
            bits_per_sample,
            block_align,
            callback,
            play_state: Arc::new(AtomicU8::new(PlayState::Stopped as u8)),
            feed_ends: Vec::new(),
            clock_freq: 0,
            sent_frames: 0,
            next_feed_id: 0,
            wake_event,
            counters,
            default_device_change_count: 0,
            device_state_change_count: 0,
            is_using_preferred_device: false,
            is_trimming_leading_silence: false,
        })
    }

    /// Open (or reopen) the audio device.
    ///
    /// If force is false and the device is already open, this is a no-op.
    pub fn open(&mut self, force: bool) -> windows::core::Result<()> {
        if self.client_slot.is_some() && !force {
            return Ok(());
        }
        // Snapshot device change counters.
        self.default_device_change_count = self
            .counters
            .default_device_change_count
            .load(Ordering::Relaxed);
        self.device_state_change_count = self
            .counters
            .device_state_change_count
            .load(Ordering::Relaxed);

        // Get the device.
        self.is_using_preferred_device = false;
        let dev = if self.endpoint_id.is_empty() {
            device::get_default_device()?
        } else {
            match device::get_preferred_device(&self.endpoint_id) {
                Ok(d) => {
                    self.is_using_preferred_device = true;
                    d
                }
                Err(_) => {
                    // Preferred device not found -- fall back to default.
                    device::get_default_device()?
                }
            }
        };

        // Build WAVEFORMATEX.
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: self.channels,
            nSamplesPerSec: self.samples_per_sec,
            wBitsPerSample: self.bits_per_sample,
            nBlockAlign: self.block_align,
            nAvgBytesPerSec: self.samples_per_sec * self.block_align as u32,
            cbSize: 0,
        };

        // Activate IAudioClient on the device.
        let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None)? };

        // Initialize the audio client in shared mode.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_SIZE,
                0,
                &format,
                None,
            )?;
        }

        let buffer_frames = unsafe { client.GetBufferSize()? };
        let render: IAudioRenderClient = unsafe { client.GetService()? };
        let clock: IAudioClock = unsafe { client.GetService()? };
        let clock_freq = unsafe { clock.GetFrequency()? };

        self.client_slot.replace(Some(client));
        self.render = Some(render);
        self.clock = Some(clock);
        self.buffer_frames = buffer_frames;
        self.clock_freq = clock_freq;
        self.set_play_state(PlayState::Stopped);

        // Disable communication ducking -- non-fatal if it fails.
        if let Err(e) = device::disable_communication_ducking(&dev) {
            eprintln!("Couldn't disable communication ducking: {:?}", e);
        }

        Ok(())
    }

    /// Feed a chunk of audio data. Returns a feed id (when `want_id` is
    /// true) that will be passed to the callback when this chunk finishes
    /// playing; otherwise returns 0.
    pub fn feed(
        &mut self,
        data: &[u8],
        want_id: bool,
    ) -> windows::core::Result<u32> {
        self.feed_inner(FeedSource::Data(data), want_id)
    }

    /// Feed `frame_count` frames of silence using `AUDCLNT_BUFFERFLAGS_SILENT`.
    ///
    /// This is what the silence keep-alive thread should call: WASAPI does
    /// not need to read the buffer (the SILENT flag tells the device to
    /// emit silence), so there's no memcpy of zero data per buffer cycle.
    /// Mirrors the C++ `WasapiPlayer::feed(null, SILENCE_BYTES, null)` path.
    pub fn feed_silence(&mut self, frame_count: u32) -> windows::core::Result<()> {
        let _ = self.feed_inner(FeedSource::Silence(frame_count), false)?;
        Ok(())
    }

    fn feed_inner(
        &mut self,
        source: FeedSource<'_>,
        want_id: bool,
    ) -> windows::core::Result<u32> {
        if self.get_play_state() == PlayState::Stopping {
            self.complete_stop();
        }

        let block_align = self.block_align as usize;
        // Work out where the data starts and how many frames to send. A
        // `Silence` source carries no bytes (`None`) and emits `n` frames via
        // the SILENT flag; a `Data` source starts as the whole chunk.
        let (mut data_slice, mut remaining_frames): (Option<&[u8]>, u32) = match source {
            FeedSource::Data(d) => (Some(d), d.len() as u32 / self.block_align as u32),
            FeedSource::Silence(n) => (None, n),
        };
        let mut should_insert_silent_frame = false;

        if let Some(raw) = data_slice {
            if self.is_trimming_leading_silence && !raw.is_empty() {
                let silence_size = silence_detect::get_leading_silence_size(
                    silence_detect::WaveFormatTag::Pcm,
                    self.bits_per_sample,
                    self.block_align,
                    raw,
                );
                if silence_size >= raw.len() {
                    // Entire chunk is silence. Keep checking next chunk.
                    // Insert one silent frame so the rest of the logic still runs.
                    should_insert_silent_frame = true;
                    remaining_frames = 1;
                    data_slice = None; // we don't copy any real data
                } else {
                    // Partial silence -- skip it.
                    let trimmed = &raw[silence_size..];
                    data_slice = Some(trimmed);
                    remaining_frames = trimmed.len() as u32 / self.block_align as u32;
                    self.is_trimming_leading_silence = false;
                    // Insert one silent frame before the trimmed audio.
                    should_insert_silent_frame = true;
                    remaining_frames += 1;
                }
            }
        }

        // Mutable pointer into the remaining data we still need to copy.
        let mut data_offset: usize = 0;

        while remaining_frames > 0 {
            // --- get padding, handling stop and device changes ---
            let mut padding_frames =
                match self.get_padding_handling_stop_or_dev_change() {
                    PaddingResult::Ok(p) => p,
                    PaddingResult::Stopped => return Ok(0),
                    PaddingResult::Err(e) => return Err(e),
                };

            if padding_frames > self.buffer_frames / 2 {
                // Wait until buffer is less than half full.
                let wait_ms =
                    self.frames_to_ms(padding_frames - self.buffer_frames / 2);
                self.wait_until_needed(wait_ms);

                // Re-check padding after waiting; the helper returns the
                // fresh value, which we use directly below (mirrors the C++
                // lambda that captures paddingFrames by reference).
                padding_frames = match self.get_padding_handling_stop_or_dev_change() {
                    PaddingResult::Ok(p) => p,
                    PaddingResult::Stopped => return Ok(0),
                    PaddingResult::Err(e) => return Err(e),
                };
            }

            let send_frames =
                remaining_frames.min(self.buffer_frames.saturating_sub(padding_frames));
            if send_frames == 0 {
                // Buffer is completely full -- wait and retry.
                self.wait_until_needed(1);
                continue;
            }
            let silent_frame_count: u32 =
                if should_insert_silent_frame { 1 } else { 0 };
            let send_bytes =
                (send_frames.saturating_sub(silent_frame_count)) as usize * block_align;

            let render = self.render.as_ref().unwrap();
            let buffer = unsafe { render.GetBuffer(send_frames)? };

            // Possibly insert a silent frame at the start.
            if should_insert_silent_frame {
                unsafe {
                    if self.bits_per_sample == 8 {
                        // 8-bit unsigned PCM: silence is 0x80.
                        std::ptr::write_bytes(buffer, 0x80, block_align);
                    } else {
                        std::ptr::write_bytes(buffer, 0, block_align);
                    }
                }
                should_insert_silent_frame = false;
            }

            let copy_dest = unsafe {
                buffer.add(silent_frame_count as usize * block_align)
            };

            if let Some(src) = data_slice {
                let src_chunk = &src[data_offset..data_offset + send_bytes];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        src_chunk.as_ptr(),
                        copy_dest,
                        send_bytes,
                    );
                    render.ReleaseBuffer(send_frames, 0)?;
                }
                data_offset += send_bytes;
            } else {
                // Null data means play silence.
                unsafe {
                    render.ReleaseBuffer(
                        send_frames,
                        AUDCLNT_BUFFERFLAGS_SILENT.0 as u32,
                    )?;
                }
            }

            if self.get_play_state() == PlayState::Stopped {
                if let Some(client) = self.client_slot.snapshot() {
                    unsafe {
                        client.Start()?;
                    }
                }
                if self.get_play_state() == PlayState::Stopping {
                    // stop() was called while we were calling Start().
                    self.complete_stop();
                    return Ok(0);
                }
                self.set_play_state(PlayState::Playing);
            }

            self.maybe_fire_callback();
            remaining_frames -= send_frames;
            self.sent_frames += send_frames;
        }

        if self.get_play_state() == PlayState::Playing {
            self.maybe_fire_callback();
        }

        let feed_id = if want_id {
            let id = self.next_feed_id;
            self.next_feed_id += 1;
            // Track callback: important to add *after* firing existing
            // callbacks so the caller already knows the id.
            let end_ms = self.frames_to_ms(self.sent_frames);
            self.feed_ends.push((id, end_ms));
            id
        } else {
            0
        };

        Ok(feed_id)
    }

    /// Stop playback. Calls `IAudioClient::Stop()` to halt audio immediately,
    /// sets play state to Stopping, and wakes any waiting feed/sync. Safe to
    /// call from any thread (delegates to the lock-free [`DeviceControlHandle`] path).
    pub fn stop(&mut self) -> windows::core::Result<()> {
        self.control_handle().stop_inner()
    }

    /// Reset our state after being stopped. Runs on the feeder thread.
    fn complete_stop(&mut self) {
        if let Some(client) = self.client_slot.snapshot() {
            if let Err(e) = unsafe { client.Reset() } {
                eprintln!("Couldn't reset stream: {:?}", e);
            }
        }
        self.next_feed_id = 0;
        self.sent_frames = 0;
        self.feed_ends.clear();
        self.set_play_state(PlayState::Stopped);
    }

    /// Wait for all buffered audio to finish playing, firing callbacks along
    /// the way.
    pub fn sync(&mut self) -> windows::core::Result<()> {
        let sent_ms = self.frames_to_ms(self.sent_frames);
        loop {
            let play_pos = self.get_play_pos();
            if play_pos >= sent_ms {
                break;
            }
            if self.get_play_state() != PlayState::Playing {
                return Ok(());
            }
            self.maybe_fire_callback();
            self.wait_until_needed(sent_ms - play_pos);
        }
        // Fire any callback right at the end of the stream.
        if self.get_play_state() == PlayState::Playing {
            self.maybe_fire_callback();
        }
        Ok(())
    }

    /// Wait for playback to finish, then stop and reset.
    pub fn idle(&mut self) -> windows::core::Result<()> {
        self.sync()?;
        self.stop()?;
        self.complete_stop();
        Ok(())
    }

    /// Pause playback without resetting position. Delegates to the lock-free
    /// [`DeviceControlHandle`] path so callers can pause even while `feed()` holds the
    /// player mutex.
    pub fn pause(&mut self) -> windows::core::Result<()> {
        self.control_handle().pause()
    }

    /// Resume playback after pause. Delegates to the lock-free [`DeviceControlHandle`]
    /// path (see [`pause`](Self::pause)).
    pub fn resume(&mut self) -> windows::core::Result<()> {
        self.control_handle().resume()
    }

    /// Set the volume for a specific channel.
    pub fn set_channel_volume(
        &mut self,
        channel: u32,
        level: f32,
    ) -> windows::core::Result<()> {
        let client = match self.client_slot.snapshot() {
            Some(c) => c,
            None => return Ok(()),
        };
        let volume_result: windows::core::Result<IAudioStreamVolume> =
            unsafe { client.GetService() };
        let volume = match volume_result {
            Ok(v) => v,
            Err(e) if is_error(&e, AUDCLNT_E_DEVICE_INVALIDATED) => {
                // Device was invalidated -- fall back to default.
                self.open(true)?;
                match self.client_slot.snapshot() {
                    Some(c) => unsafe { c.GetService()? },
                    None => return Ok(()),
                }
            }
            Err(e) => return Err(e),
        };
        unsafe {
            volume.SetChannelVolume(channel, level)?;
        }
        Ok(())
    }

    /// Enable or disable leading-silence trimming.
    pub fn start_trimming_leading_silence(&mut self, start: bool) {
        self.is_trimming_leading_silence = start;
    }

    /// Get the current play state.
    fn get_play_state(&self) -> PlayState {
        PlayState::from_u8(self.play_state.load(Ordering::Acquire))
    }

    /// Set the play state.
    fn set_play_state(&self, state: PlayState) {
        self.play_state.store(state as u8, Ordering::Release);
    }

    /// Create a [`DeviceControlHandle`] that can stop/pause/resume playback
    /// from another thread.
    ///
    /// The returned handle is stable across device reopens -- it shares the
    /// same `client_slot`, `play_state` Arc, and `wake_event` HANDLE that
    /// persist for the lifetime of this player. When `feed()` reopens the
    /// device on a device-change event, the slot is updated atomically and
    /// the handle automatically references the new client.
    pub fn control_handle(&self) -> DeviceControlHandle {
        DeviceControlHandle {
            client_slot: self.client_slot.clone(),
            play_state: self.play_state.clone(),
            wake_event: self.wake_event,
        }
    }

    // ---- private helpers ----

    /// Fire the callback for any feed_ends whose position has been reached.
    fn maybe_fire_callback(&mut self) {
        if self.callback.is_none() || self.feed_ends.is_empty() {
            return;
        }
        let play_pos = self.get_play_pos();
        let callback = self.callback.as_ref().unwrap();
        self.feed_ends.retain(|&(id, end)| {
            if play_pos >= end {
                callback(id);
                false
            } else {
                true
            }
        });
    }

    /// Convert frames to milliseconds.
    fn frames_to_ms(&self, frames: u32) -> u64 {
        frames as u64 * 1000 / self.samples_per_sec as u64
    }

    /// Get the current playback position in milliseconds.
    fn get_play_pos(&self) -> u64 {
        if let Some(ref clock) = self.clock {
            let mut pos: u64 = 0;
            let hr = unsafe { clock.GetPosition(&mut pos, None) };
            if hr.is_ok() {
                return pos * 1000 / self.clock_freq;
            }
        }
        // On error, treat as if playback has finished.
        self.frames_to_ms(self.sent_frames)
    }

    /// Wait until we need to wake up, capping at `max_wait` ms. Also wakes
    /// early if there is a pending callback sooner.
    fn wait_until_needed(&self, max_wait: u64) {
        let mut wait = max_wait;
        if let Some(&(_, feed_end)) = self.feed_ends.first() {
            let play_pos = self.get_play_pos();
            if feed_end > play_pos {
                let next_callback_time = feed_end - play_pos;
                if next_callback_time < wait {
                    wait = next_callback_time;
                }
            }
        }
        unsafe {
            let _ = WaitForSingleObject(self.wake_event, wait as u32);
        }
    }

    /// Returns true if a preferred device has become available (was previously
    /// unavailable) and we should switch to it.
    fn did_preferred_device_become_available(&self) -> bool {
        if self.is_using_preferred_device
            || self.endpoint_id.is_empty()
            || self.device_state_change_count
                == self
                    .counters
                    .device_state_change_count
                    .load(Ordering::Relaxed)
        {
            return false;
        }
        device::get_preferred_device(&self.endpoint_id).is_ok()
    }

    /// Reopen the device, fire all pending callbacks (they'll never complete
    /// on the old device), and reset sent_frames.
    fn reopen_using_new_device(&mut self) -> windows::core::Result<()> {
        self.open(true)?;
        // Fire all pending callbacks since the old stream is gone.
        if let Some(ref callback) = self.callback {
            for &(id, _) in &self.feed_ends {
                callback(id);
            }
        }
        self.feed_ends.clear();
        self.sent_frames = 0;
        Ok(())
    }

    /// Get the current padding, handling stop requests and device changes.
    /// Returns the padding frame count, or a signal to stop/abort.
    fn get_padding_handling_stop_or_dev_change(
        &mut self,
    ) -> PaddingResult {
        if self.get_play_state() == PlayState::Stopping {
            self.complete_stop();
            return PaddingResult::Stopped;
        }

        // Check for device changes.
        let need_reopen = self.did_preferred_device_become_available()
            || (!self.is_using_preferred_device
                && self.default_device_change_count
                    != self
                        .counters
                        .default_device_change_count
                        .load(Ordering::Relaxed));
        if need_reopen {
            if let Err(e) = self.reopen_using_new_device() {
                return PaddingResult::Err(e);
            }
        }

        // Get current padding.
        let client = match self.client_slot.snapshot() {
            Some(c) => c,
            None => return PaddingResult::Stopped,
        };
        match unsafe { client.GetCurrentPadding() } {
            Ok(p) => PaddingResult::Ok(p),
            Err(e)
                if is_error(&e, AUDCLNT_E_DEVICE_INVALIDATED)
                    || is_error(&e, AUDCLNT_E_NOT_INITIALIZED) =>
            {
                // Try to reopen.
                if let Err(e2) = self.reopen_using_new_device() {
                    return PaddingResult::Err(e2);
                }
                let client = match self.client_slot.snapshot() {
                    Some(c) => c,
                    None => return PaddingResult::Stopped,
                };
                match unsafe { client.GetCurrentPadding() } {
                    Ok(p) => PaddingResult::Ok(p),
                    Err(e2) => PaddingResult::Err(e2),
                }
            }
            Err(e) => PaddingResult::Err(e),
        }
    }
}

/// Result of getting the current buffer padding.
enum PaddingResult {
    Ok(u32),
    Stopped,
    Err(windows::core::Error),
}

// SAFETY: WasapiPlayerInner contains COM pointers (IAudioClient, etc.) and a
// HANDLE. COM interfaces used here are free-threaded (MTA), and we only ever
// access the player through a Mutex, so sending it to another thread is safe.
unsafe impl Send for WasapiPlayerInner {}

/// A lightweight, thread-safe handle for controlling playback from any thread
/// without acquiring the player mutex — used to `stop`, `pause`, or `resume`
/// while a blocking `feed()` holds the mutex.
///
/// It works by snapshotting the current `IAudioClient` from the shared slot and
/// driving it (`Stop()` / `Start()`) directly. The three operations differ only
/// in how they touch the two shared atomics:
/// - `stop`: `IAudioClient::Stop()`, then set play_state to Stopping (release,
///   paired with the feeder's acquire load), then signal the wake event so the
///   feed loop wakes and runs `complete_stop()` → `IAudioClient::Reset()`.
/// - `pause`/`resume`: `IAudioClient::Stop()` / `Start()` only. play_state stays
///   `Playing` and the wake event is not signalled, so the feeder keeps its
///   position (and stays parked in its backpressure wait until resume drains
///   the device).
///
/// The client slot is updated atomically by `WasapiPlayerInner::open()` on
/// device-change reopen. An operation racing against a reopen acts on whichever
/// client is current when the slot is read; calling `Stop()`/`Start()` on a
/// just-replaced client is harmless (it drives a stream no longer routed to the
/// device).
pub struct DeviceControlHandle {
    client_slot: Arc<ClientSlot>,
    play_state: Arc<AtomicU8>,
    wake_event: HANDLE,
}

// SAFETY: `ClientSlot` carries Send + Sync via its own manual impls.
// `AtomicU8` is inherently thread-safe. `HANDLE` for `SetEvent` is
// documented as thread-safe by Windows.
unsafe impl Send for DeviceControlHandle {}
unsafe impl Sync for DeviceControlHandle {}

impl DeviceControlHandle {
    /// Stop playback from any thread. This is the thread-safe equivalent of
    /// `WasapiPlayerInner::stop()`.
    pub fn stop(&self) {
        // Ignore the result -- treat the same way the inner stop() does for
        // benign HRESULTs (device invalidated / not initialised).
        let _ = self.stop_inner();
    }

    /// Stop playback and return any error from `IAudioClient::Stop()`. Used
    /// by `WasapiPlayerInner::stop()` to surface real failures while we
    /// still propagate `Result` upwards.
    pub(crate) fn stop_inner(&self) -> windows::core::Result<()> {
        // Stop the current client (if any) BEFORE flipping state, so the feeder
        // thread doesn't call Reset() before Stop() completes.
        let result = self.client_slot.snapshot().map(|c| unsafe { c.Stop() });
        self.play_state
            .store(PlayState::Stopping as u8, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.wake_event);
        }
        match result {
            // Surface real failures; a device that's already invalidated or
            // uninitialised is a benign "already stopped".
            Some(Err(e))
                if !is_error(&e, AUDCLNT_E_DEVICE_INVALIDATED)
                    && !is_error(&e, AUDCLNT_E_NOT_INITIALIZED) =>
            {
                Err(e)
            }
            _ => Ok(()),
        }
    }

    /// Pause playback from any thread without resetting position (out-of-band
    /// `IAudioClient::Stop()`; see [`if_playing`](Self::if_playing)).
    pub fn pause(&self) -> windows::core::Result<()> {
        self.if_playing(|client| unsafe { client.Stop() })
    }

    /// Resume playback after [`pause`](Self::pause), from any thread
    /// (out-of-band `IAudioClient::Start()`). This is what unblocks a `feed()`
    /// stuck in its backpressure wait, since the device begins draining again.
    pub fn resume(&self) -> windows::core::Result<()> {
        self.if_playing(|client| unsafe { client.Start() })
    }

    /// Run `op` on the current client iff playback is active. Shared by
    /// pause/resume: both snapshot the current `IAudioClient` and drive it
    /// directly (no player mutex) — which is why a paused stream can leave
    /// `feed()` blocked in its backpressure wait, still holding the mutex,
    /// without deadlocking. Unlike stop, play_state is left as `Playing` and
    /// the wake event is not signalled, so position is preserved. A no-op if
    /// stopped/stopping or if there is no live client.
    fn if_playing(
        &self,
        op: impl FnOnce(&IAudioClient) -> windows::core::Result<()>,
    ) -> windows::core::Result<()> {
        if PlayState::from_u8(self.play_state.load(Ordering::Acquire)) != PlayState::Playing {
            return Ok(());
        }
        match self.client_slot.snapshot() {
            Some(client) => op(&client),
            None => Ok(()),
        }
    }
}

impl Drop for WasapiPlayerInner {
    fn drop(&mut self) {
        if !self.wake_event.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.wake_event);
            }
        }
    }
}
