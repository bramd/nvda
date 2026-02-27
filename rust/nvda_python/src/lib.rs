#![allow(non_snake_case)]

use pyo3::prelude::*;
use pyo3::types::PyBytes;

mod wasapi;

/// Generate a beep tone as raw PCM bytes.
///
/// Parameters:
///   hz: frequency in hertz
///   length_ms: duration in milliseconds
///   left: left channel volume (0-100)
///   right: right channel volume (0-100)
///
/// Returns: bytes containing 16-bit stereo PCM audio at 44100 Hz
#[pyfunction]
#[pyo3(name = "generateBeep")]
fn generate_beep<'py>(
    py: Python<'py>,
    hz: f32,
    lengthMs: u32,
    left: u32,
    right: u32,
) -> Bound<'py, PyBytes> {
    let pcm = nvda_tones::generate_beep(hz, lengthMs, left, right);
    PyBytes::new(py, &pcm)
}

#[pymodule]
#[pyo3(name = "tones")]
mod tones_mod {
    #[pymodule_export]
    use super::generate_beep;
}

#[pymodule]
#[pyo3(name = "wasapi")]
mod wasapi_mod {
    #[pymodule_export]
    use super::wasapi::WasapiPlayer;
    #[pymodule_export]
    use super::wasapi::wasapi_startup;
    #[pymodule_export]
    use super::wasapi::silence_init;
    #[pymodule_export]
    use super::wasapi::silence_play_for;
    #[pymodule_export]
    use super::wasapi::silence_terminate;
    #[pymodule_export]
    use super::wasapi::audio_ducking_should_delay;
}

#[pymodule]
#[pyo3(name = "nvdaRust")]
mod nvda_rust {
    #[pymodule_export]
    use super::tones_mod;
    #[pymodule_export]
    use super::wasapi_mod;
}
