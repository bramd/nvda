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
/// `StopHandle` it creates, so a stop call from another thread can call
/// `IAudioClient::Stop()` directly on the current client (halting audio
/// immediately) without acquiring the player mutex. The slot is updated
/// atomically by `open()` when the device is reopened, so a stop racing
/// against a device-change reopen will call `Stop()` on whichever client
/// is current at the moment the slot is read; calling `Stop()` on a
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
            play_state: Arc::new(AtomicU8::new(PlayState::STOPPED_U8)),
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
        // Work out how many frames to send and where the data starts.
        let mut data_slice: Option<&[u8]> = match source {
            FeedSource::Data(d) => Some(d),
            FeedSource::Silence(_) => None,
        };
        let mut remaining_frames: u32;
        let mut should_insert_silent_frame = false;

        if let Some(raw) = data_slice {
            remaining_frames = raw.len() as u32 / self.block_align as u32;

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
        } else if let FeedSource::Silence(n) = source {
            // Silent feed: produce N frames of silence via the SILENT flag.
            remaining_frames = n;
        } else {
            unreachable!("data_slice is None only when source is Silence");
        }

        // Mutable pointer into the remaining data we still need to copy.
        let mut data_offset: usize = 0;

        while remaining_frames > 0 {
            // --- get padding, handling stop and device changes ---
            let padding_frames =
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

                // Re-check padding after waiting.
                match self.get_padding_handling_stop_or_dev_change() {
                    PaddingResult::Ok(_p) => { /* proceed with fresh state below */ }
                    PaddingResult::Stopped => return Ok(0),
                    PaddingResult::Err(e) => return Err(e),
                };
            }

            // Re-read padding after possible wait (the C++ code calls
            // getPaddingHandlingStopOrDevChange again in the branch above and
            // falls through with the updated paddingFrames). We do the same
            // by calling GetCurrentPadding once more.
            let padding_frames = match self.client_slot.snapshot() {
                Some(client) => unsafe { client.GetCurrentPadding()? },
                None => return Ok(0),
            };

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
    /// call from any thread (delegates to the lock-free [`StopHandle`] path).
    pub fn stop(&mut self) -> windows::core::Result<()> {
        self.stop_handle().stop_inner()
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

    /// Pause playback without resetting position.
    pub fn pause(&mut self) -> windows::core::Result<()> {
        if self.get_play_state() != PlayState::Playing {
            return Ok(());
        }
        if let Some(client) = self.client_slot.snapshot() {
            unsafe {
                client.Stop()?;
            }
        }
        Ok(())
    }

    /// Resume playback after pause.
    pub fn resume(&mut self) -> windows::core::Result<()> {
        if self.get_play_state() != PlayState::Playing {
            return Ok(());
        }
        if let Some(client) = self.client_slot.snapshot() {
            unsafe {
                client.Start()?;
            }
        }
        Ok(())
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

    /// Create a StopHandle that can stop playback from another thread.
    ///
    /// The returned handle is stable across device reopens -- it shares the
    /// same `client_slot`, `play_state` Arc, and `wake_event` HANDLE that
    /// persist for the lifetime of this player. When `feed()` reopens the
    /// device on a device-change event, the slot is updated atomically and
    /// the handle automatically references the new client.
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
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

/// A lightweight, thread-safe handle for stopping playback from any thread.
///
/// This allows `stop()` to interrupt a blocking `feed()` call without needing
/// to acquire the player mutex. It:
/// 1. Snapshots the current `IAudioClient` from the shared slot and calls
///    `IAudioClient::Stop()` directly so audio output halts immediately
///    (instead of draining the up-to-`BUFFER_MS` worth of frames already
///    queued in the device).
/// 2. Sets the atomic play_state to Stopping (release ordering, paired with
///    the feeder thread's acquire load).
/// 3. Signals the wake event so the feed loop wakes up and runs
///    `complete_stop()` which calls `IAudioClient::Reset()`.
///
/// The client slot is updated atomically by `WasapiPlayerInner::open()` on
/// device-change reopen. A stop racing against a reopen will call `Stop()`
/// on whichever client is current at the moment the slot is read; calling
/// `Stop()` on a just-replaced client is harmless (it stops a stream that
/// is no longer routed to the device).
pub struct StopHandle {
    client_slot: Arc<ClientSlot>,
    play_state: Arc<AtomicU8>,
    wake_event: HANDLE,
}

// SAFETY: `ClientSlot` carries Send + Sync via its own manual impls.
// `AtomicU8` is inherently thread-safe. `HANDLE` for `SetEvent` is
// documented as thread-safe by Windows.
unsafe impl Send for StopHandle {}
unsafe impl Sync for StopHandle {}

impl StopHandle {
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
        if let Some(client) = self.client_slot.snapshot() {
            let result = unsafe { client.Stop() };
            // Set state AFTER client.Stop() to avoid the feeder thread
            // calling Reset() before Stop() completes.
            self.play_state
                .store(PlayState::STOPPING_U8, Ordering::Release);
            unsafe {
                let _ = SetEvent(self.wake_event);
            }
            if let Err(e) = result {
                let code = e.code().0;
                if code == AUDCLNT_E_DEVICE_INVALIDATED
                    || code == AUDCLNT_E_NOT_INITIALIZED
                {
                    // Device already stopped/invalidated -- ignore.
                    return Ok(());
                }
                return Err(e);
            }
        } else {
            self.play_state
                .store(PlayState::STOPPING_U8, Ordering::Release);
            unsafe {
                let _ = SetEvent(self.wake_event);
            }
        }
        Ok(())
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
