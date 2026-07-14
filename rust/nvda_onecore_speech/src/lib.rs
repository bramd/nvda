//! Rust port of NVDA's OneCore speech engine
//! (`nvdaHelper/localWin10/oneCoreSpeech.cpp`).
//!
//! A C ABI over the WinRT `Windows.Media.SpeechSynthesis` engine, driven by
//! `source/synthDrivers/oneCore.py` via ctypes (`windll` == `__stdcall`).
//! `speak` is asynchronous: it synthesises an utterance on a background
//! thread and delivers, in one callback per utterance, a full WAV buffer +
//! a `"text:time|…"` markers string. Everything downstream (SSML, queueing,
//! marker→byte conversion, feeding the Rust WasapiPlayer) stays in Python.
//!
//! Phase 1: the token/activation state machine + the accessors. `speak`
//! (the async synthesis) lands in Phase 2.

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use windows::core::{Interface, BSTR, HSTRING};
use windows::Foundation::Collections::IVectorView;
use windows::Foundation::Metadata::ApiInformation;
use windows::Media::IMediaMarker;
use windows::Media::SpeechSynthesis::{
    SpeechAppendedSilence, SpeechPunctuationSilence, SpeechSynthesisStream,
    SpeechSynthesizer,
};
use windows::Storage::Streams::{Buffer, IBuffer, InputStreamOptions};
use windows::Win32::System::WinRT::IBufferByteAccess;

/// `void (*)(BYTE* data, int length, const wchar_t* markers)` — one call per
/// utterance with the whole WAV buffer + a `"text:time|…"` markers string,
/// or `(NULL, 0, NULL)` on failure.
pub type OcSpeechCallback =
    Option<unsafe extern "system" fn(*mut u8, i32, *const u16)>;

/// The live synth, wrapped so it can live in the global state and be used
/// from the Python thread and background callback threads. WinRT synth
/// objects are agile (the C++ shares one via `shared_ptr` across the GIL
/// thread + threadpool threads); this asserts that.
struct AgileSynth(SpeechSynthesizer);
unsafe impl Send for AgileSynth {}
unsafe impl Sync for AgileSynth {}

/// The single active activation. The "token" handed to the caller is
/// `generation` — a monotonic id — so a stale async callback or a call after
/// `terminate` is detected by comparing the token to the current generation.
struct Active {
    generation: u64,
    synth: AgileSynth,
    callback: OcSpeechCallback,
}

/// Global state, guarded like the C++ `shared_timed_mutex`:
/// `initialize`/`terminate` take the write lock; accessors + the async
/// callback take the read lock (so `terminate` blocks while a callback is in
/// flight, and a callback never fires for a terminated synth).
static STATE: RwLock<Option<Active>> = RwLock::new(None);
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Backs the `getCurrentVoiceId` / `getCurrentVoiceLanguage` returns:
    /// the C++ returned `c_str()` of a *local* hstring (a dangling pointer
    /// that only works because the caller copies immediately); we hold the
    /// last value here until the next call on this thread.
    static VOICE_STR: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
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

/// Run `f` with the active synth iff `token` is the current generation,
/// under a read lock; otherwise return `default`.
fn with_synth<R>(
    token: *mut c_void,
    default: R,
    f: impl FnOnce(&SpeechSynthesizer) -> R,
) -> R {
    let gen = token as usize as u64;
    let guard = STATE.read().unwrap();
    match guard.as_ref() {
        Some(a) if gen != 0 && a.generation == gen => f(&a.synth.0),
        _ => default,
    }
}

/// Store `s` in the per-thread buffer (NUL-terminated) and return a pointer
/// valid until the next call on this thread.
fn store_voice_str(s: &str) -> *const u16 {
    VOICE_STR.with(|buf| {
        let mut b = buf.borrow_mut();
        *b = s.encode_utf16().chain(std::iter::once(0)).collect();
        b.as_ptr()
    })
}

/// `true` iff the API can set rate/pitch/volume live (UniversalApiContract >= 5).
#[no_mangle]
pub extern "system" fn ocSpeech_supportsProsodyOptions() -> bool {
    is_universal_api_contract(5, 0)
}

/// `true` iff punctuation-silence control is available (contract >= 6).
#[no_mangle]
pub extern "system" fn ocSpeech_supportsPunctuationSilence() -> bool {
    is_universal_api_contract(6, 0)
}

/// Activate the engine with `callback` and return an opaque token. Only one
/// activation may be live; returns NULL if one already is (the caller must
/// `terminate` first) or if the synth can't be created.
///
/// # Safety
/// `callback` must stay valid until the returned token is terminated.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_initialize(
    callback: OcSpeechCallback,
) -> *mut c_void {
    let mut guard = STATE.write().unwrap();
    if guard.is_some() {
        // Matches the C++ `activate` requiring a terminated state first.
        return core::ptr::null_mut();
    }
    let synth = match SpeechSynthesizer::new() {
        Ok(s) => s,
        Err(_) => return core::ptr::null_mut(),
    };
    prevent_end_utterance_silence(&synth);
    let generation = NEXT_GEN.fetch_add(1, Ordering::Relaxed);
    *guard = Some(Active {
        generation,
        synth: AgileSynth(synth),
        callback,
    });
    generation as usize as *mut c_void
}

/// Invalidate `token` and drop the synth + callback. Blocks (via the write
/// lock) until any in-flight callback finishes.
///
/// # Safety
/// `token` must be a token from `ocSpeech_initialize`.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_terminate(token: *mut c_void) {
    let gen = token as usize as u64;
    let mut guard = STATE.write().unwrap();
    if guard.as_ref().is_some_and(|a| a.generation == gen) {
        *guard = None;
    }
}

/// Available voices as `"Id:Language:DisplayName|…"`, `SysAllocString`'d
/// (the caller frees it). Empty BSTR on an invalid token.
///
/// # Safety
/// Returns an owned `BSTR`; the caller must `SysFreeString` it.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_getVoices(token: *mut c_void) -> *mut u16 {
    let s = with_synth(token, String::new(), |_| voices_string());
    let wide: Vec<u16> = s.encode_utf16().collect();
    BSTR::from_wide(&wide).unwrap_or_default().into_raw() as *mut u16
}

// `AllVoices` is a static WinRT property (the system voice list), matching the
// C++ `synth->AllVoices()`.
fn voices_string() -> String {
    let mut out = String::new();
    let Ok(all) = SpeechSynthesizer::AllVoices() else {
        return out;
    };
    let n = all.Size().unwrap_or(0);
    for i in 0..n {
        let Ok(v) = all.GetAt(i) else { continue };
        let id = v.Id().map(|h| h.to_string_lossy()).unwrap_or_default();
        let lang = v.Language().map(|h| h.to_string_lossy()).unwrap_or_default();
        let name = v.DisplayName().map(|h| h.to_string_lossy()).unwrap_or_default();
        out.push_str(&id);
        out.push(':');
        out.push_str(&lang);
        out.push(':');
        out.push_str(&name);
        if i != n - 1 {
            out.push('|');
        }
    }
    out
}

/// The current voice's id (pointer valid until the next call on this thread).
///
/// # Safety
/// The returned pointer is only valid until the next `getCurrentVoiceId` /
/// `getCurrentVoiceLanguage` call on the same thread; copy it immediately.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_getCurrentVoiceId(
    token: *mut c_void,
) -> *const u16 {
    let s = with_synth(token, String::new(), |synth| {
        synth
            .Voice()
            .and_then(|v| v.Id())
            .map(|h| h.to_string_lossy())
            .unwrap_or_default()
    });
    store_voice_str(&s)
}

/// The current voice's language (see `getCurrentVoiceId` for lifetime).
///
/// # Safety
/// Same as `ocSpeech_getCurrentVoiceId`.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_getCurrentVoiceLanguage(
    token: *mut c_void,
) -> *const u16 {
    let s = with_synth(token, String::new(), |synth| {
        synth
            .Voice()
            .and_then(|v| v.Language())
            .map(|h| h.to_string_lossy())
            .unwrap_or_default()
    });
    store_voice_str(&s)
}

/// Set the voice by index into `AllVoices()`.
///
/// # Safety
/// `token` must be a valid token.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_setVoice(token: *mut c_void, index: i32) {
    with_synth(token, (), |synth| {
        if let Ok(voices) = SpeechSynthesizer::AllVoices() {
            if let Ok(v) = voices.GetAt(index as u32) {
                let _ = synth.SetVoice(&v);
            }
        }
    });
}

macro_rules! option_getter {
    ($name:ident, $method:ident) => {
        /// # Safety
        /// `token` must be a valid token.
        #[no_mangle]
        pub unsafe extern "system" fn $name(token: *mut c_void) -> f64 {
            with_synth(token, 0.0, |s| {
                s.Options().and_then(|o| o.$method()).unwrap_or(0.0)
            })
        }
    };
}
macro_rules! option_setter {
    ($name:ident, $method:ident) => {
        /// # Safety
        /// `token` must be a valid token.
        #[no_mangle]
        pub unsafe extern "system" fn $name(token: *mut c_void, value: f64) {
            with_synth(token, (), |s| {
                let _ = s.Options().and_then(|o| o.$method(value));
            });
        }
    };
}

option_getter!(ocSpeech_getPitch, AudioPitch);
option_setter!(ocSpeech_setPitch, SetAudioPitch);
option_getter!(ocSpeech_getVolume, AudioVolume);
option_setter!(ocSpeech_setVolume, SetAudioVolume);
option_getter!(ocSpeech_getRate, SpeakingRate);
option_setter!(ocSpeech_setRate, SetSpeakingRate);

/// `true` iff punctuation silence is at the default (spoken) level.
///
/// # Safety
/// `token` must be a valid token.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_getPunctuationSilence(
    token: *mut c_void,
) -> bool {
    with_synth(token, false, |s| {
        s.Options()
            .and_then(|o| o.PunctuationSilence())
            .map(|p| p == SpeechPunctuationSilence::Default)
            .unwrap_or(false)
    })
}

/// Set punctuation silence: default (spoken) vs. min.
///
/// # Safety
/// `token` must be a valid token.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_setPunctuationSilence(
    token: *mut c_void,
    silence: bool,
) {
    with_synth(token, (), |s| {
        let mode = if silence {
            SpeechPunctuationSilence::Default
        } else {
            SpeechPunctuationSilence::Min
        };
        let _ = s.Options().and_then(|o| o.SetPunctuationSilence(mode));
    });
}

/// Assert `Send` for the WinRT objects moved onto the synthesis thread —
/// agile, as the C++ relies on when it resumes the coroutine on a threadpool
/// thread.
struct SendWrap<T>(T);
unsafe impl<T> Send for SendWrap<T> {}

/// Read a NUL-terminated wide string into an `HSTRING`.
unsafe fn wide_to_hstring(p: *const u16) -> HSTRING {
    if p.is_null() {
        return HSTRING::new();
    }
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    HSTRING::from_wide(unsafe { core::slice::from_raw_parts(p, len) })
        .unwrap_or_default()
}

/// Synthesise `text` (SSML) asynchronously and deliver the result through the
/// callback. Returns immediately. Port of C++ `ocSpeech_speak` + the
/// `fire_and_forget speak` coroutine.
///
/// # Safety
/// `token` must be a valid token; `text` a NUL-terminated wide SSML string.
#[no_mangle]
pub unsafe extern "system" fn ocSpeech_speak(
    token: *mut c_void,
    text: *const u16,
) {
    // Snapshot (generation, synth clone, callback) under a brief read lock,
    // then release it before the slow WinRT async runs.
    let gen = token as usize as u64;
    let snapshot = {
        let guard = STATE.read().unwrap();
        match guard.as_ref() {
            Some(a) if gen != 0 && a.generation == gen => {
                Some((a.synth.0.clone(), a.callback))
            }
            _ => None,
        }
    };
    let Some((synth, callback)) = snapshot else {
        return;
    };
    let ssml = unsafe { wide_to_hstring(text) };
    let payload = SendWrap((synth, ssml));
    std::thread::spawn(move || {
        let SendWrap((synth, ssml)) = payload;
        let result = synthesize(&synth, &ssml).ok();
        fire_callback(gen, callback, result);
    });
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

/// Fire the callback, re-validating the token under a read lock first — so a
/// concurrent `terminate` (write lock) blocks until this returns, and a
/// callback for a superseded/terminated synth is dropped (C++
/// `protectedCallback_`).
fn fire_callback(
    gen: u64,
    callback: OcSpeechCallback,
    result: Option<(IBuffer, HSTRING)>,
) {
    let guard = STATE.read().unwrap();
    if guard.as_ref().is_none_or(|a| a.generation != gen) {
        return;
    }
    let Some(cb) = callback else { return };
    match result {
        Some((buffer, markers)) => {
            let (ptr, len) = buffer_bytes(&buffer);
            let mut mwide = markers.as_wide().to_vec();
            mwide.push(0);
            // buffer + mwide are held alive across the call.
            unsafe { cb(ptr, len, mwide.as_ptr()) };
        }
        None => unsafe { cb(core::ptr::null_mut(), 0, core::ptr::null()) },
    }
    // `guard` (read lock) stays held across the callback, blocking terminate.
    drop(guard);
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
