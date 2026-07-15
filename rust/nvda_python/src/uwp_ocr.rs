#![allow(non_snake_case)]

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

/// Available OCR recognition language tags (e.g. `["de-de", "en-us"]`).
#[pyfunction]
#[pyo3(name = "getLanguages")]
pub fn get_languages() -> Vec<String> {
    nvda_uwp_ocr::get_languages()
}

/// Recognise text in a BGRA8 image and deliver the result asynchronously.
///
/// `image` is `width * height` `RGBQUAD`s (BGRA, 4 bytes each) as a `bytes`
/// object. The engine for `language` is created synchronously; a
/// `RuntimeError` is raised if OCR is unavailable for that language. Otherwise
/// recognition runs on a background thread and `callback` is invoked (on that
/// thread, with the GIL held) with the JSON result string, or `None` on
/// recognition failure.
#[pyfunction]
pub fn recognize(
    language: &str,
    image: &[u8],
    width: u32,
    height: u32,
    callback: Py<PyAny>,
) -> PyResult<()> {
    let cb = move |json: Option<&str>| {
        Python::attach(|py| {
            if let Err(e) = callback.call1(py, (json,)) {
                log::warn!("UWP OCR callback raised: {e:?}");
            }
        });
    };
    if nvda_uwp_ocr::recognize(language, image, width, height, Box::new(cb)) {
        Ok(())
    } else {
        Err(PyRuntimeError::new_err("UWP OCR initialization failed"))
    }
}
