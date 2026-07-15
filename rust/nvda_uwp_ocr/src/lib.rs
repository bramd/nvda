//! Rust engine for NVDA's UWP OCR, wrapping the WinRT `Windows.Media.Ocr` API.
//!
//! This crate is pure Rust with no PyO3 dependency; the `nvda_python` crate
//! exposes it to Python as `nvdaRust.uwp_ocr` (see
//! `nvda_python/src/uwp_ocr.rs`). It replaced the earlier C-ABI seam that
//! `source/contentRecog/uwpOcr.py` drove via ctypes through
//! nvdaHelperLocalWin10.dll.
//!
//! Recognition is asynchronous: [`recognize`] creates the engine synchronously
//! (so an unavailable language is reported at once) and then runs the recognise
//! off the caller's thread, delivering the result (a JSON string of
//! lines/words/bounding-rects, or `None` on failure) through the callback.

#![allow(non_snake_case)]

use windows::core::{Interface, HSTRING};
use windows::Data::Json::{IJsonValue, JsonArray, JsonObject, JsonValue};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::System::WinRT::IBufferByteAccess;

/// Callback invoked once with the recognition result: `Some(json)` (an array of
/// lines, each an array of words `{x, y, width, height, text}`) or `None` on
/// failure. Called on the recognition worker thread.
pub type ResultCallback = Box<dyn Fn(Option<&str>) + Send>;

/// Assert `Send` for the WinRT objects moved onto the recognition thread — the
/// engine + bitmap are agile.
struct SendWrap<T>(T);
unsafe impl<T> Send for SendWrap<T> {}

/// Available OCR language tags (e.g. `["de-de", "en-us"]`).
pub fn get_languages() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() {
        for lang in langs {
            if let Ok(tag) = lang.LanguageTag() {
                out.push(tag.to_string_lossy());
            }
        }
    }
    out
}

/// Recognise text in a BGRA8 image (`width` x `height`, `RGBQUAD` rows).
///
/// Creates the OCR engine for `language` synchronously and returns `false` if
/// OCR is unavailable for that language (or the image can't be prepared). On
/// `true`, recognition runs on a background thread and `callback` fires with
/// the JSON result (or `None` on recognition failure).
pub fn recognize(
    language: &str,
    image: &[u8],
    width: u32,
    height: u32,
    callback: ResultCallback,
) -> bool {
    let lang = match Language::CreateLanguage(&HSTRING::from(language)) {
        Ok(l) => l,
        Err(_) => return false,
    };
    let engine = match OcrEngine::TryCreateFromLanguage(&lang) {
        Ok(e) if !e.as_raw().is_null() => e,
        _ => return false,
    };
    let bitmap = match build_bitmap(image, width, height) {
        Ok(b) => b,
        Err(_) => return false,
    };

    // The engine + bitmap (agile WinRT) move to a background thread; run the
    // async recognise there and fire the callback with the JSON.
    let payload = SendWrap((engine, bitmap));
    std::thread::spawn(move || {
        let SendWrap((engine, bitmap)) = payload;
        match recognize_to_json(&engine, &bitmap) {
            Ok(hs) => callback(Some(&hs.to_string_lossy())),
            Err(_) => callback(None),
        }
    });
    true
}

/// Copy the BGRA image into a WinRT `SoftwareBitmap`.
fn build_bitmap(
    image: &[u8],
    width: u32,
    height: u32,
) -> windows::core::Result<SoftwareBitmap> {
    let num_bytes = 4u32.saturating_mul(width).saturating_mul(height);
    let buf = Buffer::Create(num_bytes)?;
    let bba: IBufferByteAccess = buf.cast()?;
    let dst = unsafe { bba.Buffer()? };
    // Copy at most what the caller provided, so a short slice can't read past
    // the Python-owned buffer.
    let n = (num_bytes as usize).min(image.len());
    unsafe {
        core::ptr::copy_nonoverlapping(image.as_ptr(), dst, n);
    }
    buf.SetLength(num_bytes)?;
    // windows-rs projects only the 4-arg overload (no alpha mode); the OCR
    // input is an opaque screen capture, so the alpha mode is irrelevant.
    SoftwareBitmap::CreateCopyFromBuffer(
        &buf,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
}

/// Run recognition and build the JSON: an array of lines, each an array of
/// words `{x, y, width, height, text}`.
fn recognize_to_json(
    engine: &OcrEngine,
    bitmap: &SoftwareBitmap,
) -> windows::core::Result<HSTRING> {
    let result = engine.RecognizeAsync(bitmap)?.get()?;
    let j_lines = JsonArray::new()?;
    for line in result.Lines()? {
        let j_words = JsonArray::new()?;
        for word in line.Words()? {
            let j_word = JsonObject::new()?;
            let rect = word.BoundingRect()?;
            let num = |v: f32| -> windows::core::Result<IJsonValue> {
                JsonValue::CreateNumberValue(v as f64)?.cast()
            };
            j_word.Insert(&HSTRING::from("x"), &num(rect.X)?)?;
            j_word.Insert(&HSTRING::from("y"), &num(rect.Y)?)?;
            j_word.Insert(&HSTRING::from("width"), &num(rect.Width)?)?;
            j_word.Insert(&HSTRING::from("height"), &num(rect.Height)?)?;
            let text: IJsonValue =
                JsonValue::CreateStringValue(&word.Text()?)?.cast()?;
            j_word.Insert(&HSTRING::from("text"), &text)?;
            j_words.Append(&j_word.cast::<IJsonValue>()?)?;
        }
        j_lines.Append(&j_words.cast::<IJsonValue>()?)?;
    }
    j_lines.Stringify()
}
