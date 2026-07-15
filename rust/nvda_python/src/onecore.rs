#![allow(non_snake_case)]

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use nvda_onecore_speech::OcSpeech as InnerOcSpeech;

fn to_os_error(e: windows::core::Error) -> PyErr {
    pyo3::exceptions::PyOSError::new_err(format!(
        "OneCore speech error: {} (0x{:08X})",
        e.message(),
        e.code().0 as u32
    ))
}

/// PyO3 wrapper around the OneCore speech engine. One instance == one
/// activation; dropping it (Python GC / setting the reference to `None`) stops
/// the synthesis worker.
///
/// `speak` is asynchronous: the WAV buffer + markers arrive via the `callback`
/// passed to the constructor, invoked on the worker thread with the GIL held
/// (`Python::attach`). The callback receives `(wav: bytes | None, markers:
/// str)` -- `None` for `wav` signals a synthesis failure. This mirrors the
/// worker-thread callback the old ctypes `CFUNCTYPE` used, but hands over a
/// `bytes` object instead of a raw pointer + length.
#[pyclass]
pub struct OcSpeech {
    inner: InnerOcSpeech,
}

#[pymethods]
impl OcSpeech {
    #[new]
    fn new(callback: Py<PyAny>) -> PyResult<Self> {
        let cb = move |wav: Option<&[u8]>, markers: &str| {
            // Runs on the synthesis worker thread; acquire the GIL to call the
            // Python callback (the same threading the old CFUNCTYPE used).
            Python::attach(|py| {
                let data = wav.map(|w| PyBytes::new(py, w));
                if let Err(e) = callback.call1(py, (data, markers)) {
                    log::warn!("OneCore speech callback raised: {e:?}");
                }
            });
        };
        let inner = InnerOcSpeech::new(Box::new(cb)).map_err(to_os_error)?;
        Ok(Self { inner })
    }

    fn speak(&self, ssml: &str) {
        self.inner.speak(ssml);
    }

    fn getVoices(&self) -> Vec<String> {
        self.inner.voices()
    }

    fn getCurrentVoiceId(&self) -> String {
        self.inner.current_voice_id()
    }

    fn getCurrentVoiceLanguage(&self) -> String {
        self.inner.current_voice_language()
    }

    fn setVoice(&self, index: u32) {
        self.inner.set_voice(index);
    }

    fn getPitch(&self) -> f64 {
        self.inner.pitch()
    }
    fn setPitch(&self, value: f64) {
        self.inner.set_pitch(value);
    }
    fn getVolume(&self) -> f64 {
        self.inner.volume()
    }
    fn setVolume(&self, value: f64) {
        self.inner.set_volume(value);
    }
    fn getRate(&self) -> f64 {
        self.inner.rate()
    }
    fn setRate(&self, value: f64) {
        self.inner.set_rate(value);
    }

    fn getPunctuationSilence(&self) -> bool {
        self.inner.punctuation_silence()
    }
    fn setPunctuationSilence(&self, silence: bool) {
        self.inner.set_punctuation_silence(silence);
    }
}

/// `True` iff rate/pitch/volume can be set live (UniversalApiContract >= 5).
/// Free function because it needs no activation.
#[pyfunction]
#[pyo3(name = "supportsProsodyOptions")]
pub fn supports_prosody_options() -> bool {
    nvda_onecore_speech::supports_prosody_options()
}

/// `True` iff punctuation-silence control is available (contract >= 6).
#[pyfunction]
#[pyo3(name = "supportsPunctuationSilence")]
pub fn supports_punctuation_silence() -> bool {
    nvda_onecore_speech::supports_punctuation_silence()
}
