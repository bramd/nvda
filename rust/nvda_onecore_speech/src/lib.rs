//! Rust engine for NVDA's Windows OneCore speech, wrapping the WinRT
//! `Windows.Media.SpeechSynthesis` API.
//!
//! This crate is pure Rust with no PyO3 dependency; the `nvda_python` crate
//! exposes it to Python as a `#[pyclass]` (`nvdaRust.onecore.OcSpeech`, see
//! `nvda_python/src/onecore.rs`). It replaced the earlier C-ABI seam that
//! `source/synthDrivers/oneCore.py` drove via ctypes.
//!
//! `speak` is asynchronous: it synthesises an utterance on a dedicated worker
//! thread and delivers, in one callback per utterance, a full WAV buffer + a
//! `"text:time|…"` markers string. Everything downstream (SSML, queueing,
//! marker→byte conversion, feeding the WasapiPlayer) stays in Python.

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use windows::core::{Interface, HSTRING};
use windows::Foundation::Collections::IVectorView;
use windows::Foundation::Metadata::ApiInformation;
use windows::Media::IMediaMarker;
use windows::Media::SpeechSynthesis::{
    SpeechAppendedSilence, SpeechPunctuationSilence, SpeechSynthesisStream,
    SpeechSynthesizer,
};
use windows::Storage::Streams::{Buffer, IBuffer, InputStreamOptions};
use windows::Win32::System::WinRT::IBufferByteAccess;

/// Callback invoked once per utterance with the synthesized result: `Some`
/// (a complete WAV buffer) on success or `None` on failure, plus the
/// `"text:time|…"` markers string. Called on the worker thread.
pub type ResultCallback = Box<dyn Fn(Option<&[u8]>, &str) + Send>;

/// The live synth, made Send + Sync so it can be shared between the Python
/// (accessor) thread and the background synthesis worker. WinRT synth objects
/// are agile (the C++ shared one via `shared_ptr` across the GIL thread + a
/// threadpool thread); this asserts that.
struct AgileSynth(SpeechSynthesizer);
unsafe impl Send for AgileSynth {}
unsafe impl Sync for AgileSynth {}

/// The OneCore speech engine: one activation owning a WinRT synthesizer and a
/// dedicated synthesis worker thread. Dropping it stops the worker.
pub struct OcSpeech {
    synth: AgileSynth,
    /// SSML sender to the worker; `None` after the engine is dropped.
    tx: Option<Sender<HSTRING>>,
    /// Set on teardown so an in-flight synthesis skips its callback.
    cancelled: Arc<AtomicBool>,
}

/// Generate the prosody getter/setter pair for a `SpeechSynthesizerOptions`
/// property (rate/pitch/volume). Kept as a macro to avoid naming the options
/// type. Only meaningful when [`supports_prosody_options`] is true.
macro_rules! option_accessors {
    ($get:ident, $set:ident, $getMethod:ident, $setMethod:ident) => {
        pub fn $get(&self) -> f64 {
            self.synth.0.Options().and_then(|o| o.$getMethod()).unwrap_or(0.0)
        }
        pub fn $set(&self, value: f64) {
            let _ = self.synth.0.Options().and_then(|o| o.$setMethod(value));
        }
    };
}

impl OcSpeech {
    /// Create the engine. `callback` is invoked (on the worker thread) once per
    /// [`speak`](Self::speak) with the WAV buffer + markers.
    pub fn new(callback: ResultCallback) -> windows::core::Result<Self> {
        let synth = SpeechSynthesizer::new()?;
        prevent_end_utterance_silence(&synth);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<HSTRING>();

        // The worker owns its own clone of the (agile) synth so it stays alive
        // for the synthesis even if the OcSpeech is dropped mid-utterance.
        let worker_synth = AgileSynth(synth.clone());
        let worker_cancelled = Arc::clone(&cancelled);
        // Detached (never joined): the worker may be inside `callback`
        // acquiring Python's GIL, and joining while the dropping thread holds
        // the GIL would deadlock. A dropped sender ends the loop; the
        // `cancelled` flag drops the result of any synthesis already running.
        std::thread::spawn(move || {
            let AgileSynth(synth) = worker_synth;
            while let Ok(ssml) = rx.recv() {
                if worker_cancelled.load(Ordering::Acquire) {
                    continue;
                }
                let result = synthesize(&synth, &ssml).ok();
                if worker_cancelled.load(Ordering::Acquire) {
                    continue;
                }
                match &result {
                    Some((buffer, markers)) => {
                        let (ptr, len) = buffer_bytes(buffer);
                        let wav = if ptr.is_null() {
                            None
                        } else {
                            // SAFETY: ptr/len describe the live IBuffer; the
                            // slice does not outlive `buffer` (kept in `result`).
                            Some(unsafe {
                                std::slice::from_raw_parts(ptr, len as usize)
                            })
                        };
                        callback(wav, &markers.to_string_lossy());
                    }
                    None => callback(None, ""),
                }
            }
        });

        Ok(Self {
            synth: AgileSynth(synth),
            tx: Some(tx),
            cancelled,
        })
    }

    /// Queue an SSML utterance for asynchronous synthesis. Returns immediately;
    /// the result arrives via the callback passed to [`new`](Self::new).
    pub fn speak(&self, ssml: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(HSTRING::from(ssml));
        }
    }

    // --- voice selection ---

    /// Available voices as `"Id:Language:DisplayName"` strings (the system
    /// voice list; may include uninstalled/broken voices the caller filters).
    pub fn voices(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(all) = SpeechSynthesizer::AllVoices() else {
            return out;
        };
        for i in 0..all.Size().unwrap_or(0) {
            let Ok(v) = all.GetAt(i) else { continue };
            let id = v.Id().map(|h| h.to_string_lossy()).unwrap_or_default();
            let lang = v.Language().map(|h| h.to_string_lossy()).unwrap_or_default();
            let name = v.DisplayName().map(|h| h.to_string_lossy()).unwrap_or_default();
            out.push(format!("{id}:{lang}:{name}"));
        }
        out
    }

    pub fn current_voice_id(&self) -> String {
        self.voice_field(|v| v.Id())
    }

    pub fn current_voice_language(&self) -> String {
        self.voice_field(|v| v.Language())
    }

    /// Select the voice by index into `AllVoices()`.
    pub fn set_voice(&self, index: u32) {
        if let Ok(voices) = SpeechSynthesizer::AllVoices() {
            if let Ok(v) = voices.GetAt(index) {
                let _ = self.synth.0.SetVoice(&v);
            }
        }
    }

    fn voice_field(
        &self,
        field: impl Fn(
            &windows::Media::SpeechSynthesis::VoiceInformation,
        ) -> windows::core::Result<HSTRING>,
    ) -> String {
        self.synth
            .0
            .Voice()
            .and_then(|v| field(&v))
            .map(|h| h.to_string_lossy())
            .unwrap_or_default()
    }

    // --- prosody (only effective when supports_prosody_options()) ---

    option_accessors!(pitch, set_pitch, AudioPitch, SetAudioPitch);
    option_accessors!(volume, set_volume, AudioVolume, SetAudioVolume);
    option_accessors!(rate, set_rate, SpeakingRate, SetSpeakingRate);

    /// `true` iff punctuation silence is at the default (spoken) level.
    pub fn punctuation_silence(&self) -> bool {
        self.synth
            .0
            .Options()
            .and_then(|o| o.PunctuationSilence())
            .map(|p| p == SpeechPunctuationSilence::Default)
            .unwrap_or(false)
    }

    /// Set punctuation silence: default (spoken) vs. min.
    pub fn set_punctuation_silence(&self, silence: bool) {
        let mode = if silence {
            SpeechPunctuationSilence::Default
        } else {
            SpeechPunctuationSilence::Min
        };
        let _ = self.synth.0.Options().and_then(|o| o.SetPunctuationSilence(mode));
    }
}

impl Drop for OcSpeech {
    fn drop(&mut self) {
        // Stop the worker without joining (see the spawn comment): mark
        // cancelled so an in-flight synthesis skips its callback, and drop the
        // sender so the worker's recv() returns Err and it exits.
        self.cancelled.store(true, Ordering::Release);
        self.tx = None;
    }
}

/// `true` iff the API can set rate/pitch/volume live (UniversalApiContract >= 5).
pub fn supports_prosody_options() -> bool {
    is_universal_api_contract(5, 0)
}

/// `true` iff punctuation-silence control is available (contract >= 6).
pub fn supports_punctuation_silence() -> bool {
    is_universal_api_contract(6, 0)
}

fn is_universal_api_contract(major: u16, minor: u16) -> bool {
    ApiInformation::IsApiContractPresentByMajorAndMinor(
        &HSTRING::from("Windows.Foundation.UniversalApiContract"),
        major,
        minor,
    )
    .unwrap_or(false)
}

fn prevent_end_utterance_silence(synth: &SpeechSynthesizer) {
    // OneCore appends a long silence per utterance; disable it where the API
    // allows (UniversalApiContract >= 6.0).
    if is_universal_api_contract(6, 0) {
        if let Ok(opts) = synth.Options() {
            let _ = opts.SetAppendedSilence(SpeechAppendedSilence::Min);
        }
    }
}

/// Synthesise to a stream and read the whole WAV buffer + markers.
fn synthesize(
    synth: &SpeechSynthesizer,
    ssml: &HSTRING,
) -> windows::core::Result<(IBuffer, HSTRING)> {
    let stream: SpeechSynthesisStream =
        synth.SynthesizeSsmlToStreamAsync(ssml)?.get()?;
    // Size() is 64-bit but a Buffer is 32-bit; real utterances fit.
    let size = stream.Size()? as u32;
    let markers = markers_string(&stream.Markers()?);
    let buffer = Buffer::Create(size)?;
    let read = stream
        .ReadAsync(&buffer, size, InputStreamOptions::None)?
        .get()?;
    Ok((read, markers))
}

/// Build the `"text:time|text:time|…"` markers string (time in 100 ns ticks),
/// as C++ `createMarkersString_`.
fn markers_string(markers: &IVectorView<IMediaMarker>) -> HSTRING {
    let mut s = String::new();
    let n = markers.Size().unwrap_or(0);
    for i in 0..n {
        let Ok(m) = markers.GetAt(i) else { continue };
        if i != 0 {
            s.push('|');
        }
        let text = m.Text().map(|h| h.to_string_lossy()).unwrap_or_default();
        let time = m.Time().map(|t| t.Duration).unwrap_or(0);
        s.push_str(&text);
        s.push(':');
        s.push_str(&time.to_string());
    }
    HSTRING::from(s)
}

/// Raw byte pointer + length of a WinRT `IBuffer` (via `IBufferByteAccess`).
fn buffer_bytes(buffer: &IBuffer) -> (*mut u8, i32) {
    let len = buffer.Length().unwrap_or(0) as i32;
    match buffer.cast::<IBufferByteAccess>() {
        Ok(bba) => match unsafe { bba.Buffer() } {
            Ok(ptr) => (ptr, len),
            Err(_) => (core::ptr::null_mut(), 0),
        },
        Err(_) => (core::ptr::null_mut(), 0),
    }
}
