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

// Crash dump — thin wrapper around nvda_crashdump.
#[pyfunction]
#[pyo3(name = "writeCrashDump")]
fn write_crash_dump(path: &str, exception_pointers: usize) -> bool {
    nvda_crashdump::write_crash_dump(path, exception_pointers)
}

// OLE helpers — thin wrappers around nvda_ole. The COM IUnknown is passed
// from Python as an integer pointer:
//     ptr = ctypes.cast(comObj, ctypes.c_void_p).value
// HRESULT errors are mapped to PyOSError.
#[pyfunction]
#[pyo3(name = "getOleClipboardText")]
fn get_ole_clipboard_text(unknown: usize) -> PyResult<String> {
    nvda_ole::get_clipboard_text(unknown).map_err(|hr| {
        pyo3::exceptions::PyOSError::new_err(format!("HRESULT 0x{:08x}", hr as u32))
    })
}

#[pyfunction]
#[pyo3(name = "getOleUserType")]
fn get_ole_user_type(unknown: usize, flags: u32) -> PyResult<String> {
    nvda_ole::get_user_type(unknown, flags).map_err(|hr| {
        pyo3::exceptions::PyOSError::new_err(format!("HRESULT 0x{:08x}", hr as u32))
    })
}

// Screen curtain — thin wrapper around nvda_screen_curtain.
#[pyfunction]
#[pyo3(name = "isScreenFullyBlack")]
fn is_screen_fully_black() -> bool {
    nvda_screen_curtain::is_screen_fully_black()
}

#[pymodule]
#[pyo3(name = "crashdump")]
mod crashdump_mod {
    #[pymodule_export]
    use super::write_crash_dump;
}

#[pymodule]
#[pyo3(name = "ole")]
mod ole_mod {
    #[pymodule_export]
    use super::get_ole_clipboard_text;
    #[pymodule_export]
    use super::get_ole_user_type;
}

#[pymodule]
#[pyo3(name = "screen_curtain")]
mod screen_curtain_mod {
    #[pymodule_export]
    use super::is_screen_fully_black;
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
    #[pymodule_init]
    fn init(_m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        // Forward Rust `log` macros to Python's `logging` module. NVDA's root
        // logger handlers pick up child loggers (named after the Rust crate)
        // automatically via Python's logging hierarchy.
        pyo3_log::init();
        Ok(())
    }

    #[pymodule_export]
    use super::crashdump_mod;
    #[pymodule_export]
    use super::ole_mod;
    #[pymodule_export]
    use super::screen_curtain_mod;
    #[pymodule_export]
    use super::text_mod;
    #[pymodule_export]
    use super::tones_mod;
    #[pymodule_export]
    use super::wasapi_mod;
}
