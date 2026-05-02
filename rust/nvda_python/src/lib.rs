#![allow(non_snake_case)]

use pyo3::prelude::*;
use pyo3::types::PyBytes;

mod wasapi;

/// Generate a beep tone as raw PCM bytes.
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

// Text segmentation functions — thin wrappers around nvda_text.
#[pyfunction]
#[pyo3(name = "splitAtCharacterBoundaries")]
fn split_at_character_boundaries(text_input: &str) -> Vec<String> {
    nvda_text::split_at_character_boundaries(text_input)
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

#[pyfunction]
#[pyo3(name = "calculateCharacterOffsets")]
fn calculate_character_offsets(text_input: &str, offset: usize) -> (usize, usize) {
    nvda_text::character_offsets(text_input, offset)
}

#[pyfunction]
#[pyo3(name = "calculateWordOffsets")]
fn calculate_word_offsets(text_input: &str, offset: usize) -> (usize, usize) {
    nvda_text::word_offsets(text_input, offset)
}

#[pymodule]
#[pyo3(name = "tones")]
mod tones_mod {
    #[pymodule_export]
    use super::generate_beep;
}

#[pymodule]
#[pyo3(name = "text")]
mod text_mod {
    #[pymodule_export]
    use super::split_at_character_boundaries;
    #[pymodule_export]
    use super::calculate_character_offsets;
    #[pymodule_export]
    use super::calculate_word_offsets;
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
    use super::text_mod;
    #[pymodule_export]
    use super::tones_mod;
    #[pymodule_export]
    use super::wasapi_mod;
}
