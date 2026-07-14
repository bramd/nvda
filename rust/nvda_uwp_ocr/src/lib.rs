//! Rust port of NVDA's UWP OCR bridge
//! (`nvdaHelper/localWin10/uwpOcr.cpp`).
//!
//! A thin C ABI over the WinRT `Windows.Media.Ocr` engine, called from
//! `source/contentRecog/uwpOcr.py` via ctypes (`windll`, i.e. `__stdcall`).
//! Recognition is asynchronous: `uwpOcr_recognize` returns immediately and
//! the result (a JSON string of lines/words/bounding-rects) is delivered
//! later, on a background thread, through the caller's callback.
//!
//! The C++ used a C++/WinRT coroutine (`fire_and_forget` +
//! `co_await resume_background()` + `co_await RecognizeAsync`); here that is
//! a plain background thread that blocks on the async op's `.get()` — the
//! same "recognize off the caller's thread, fire the callback when done"
//! behaviour, without the coroutine machinery.

#![allow(non_snake_case)]

use core::ffi::c_void;

use windows::core::{Interface, BSTR, HSTRING};
use windows::Data::Json::{IJsonValue, JsonArray, JsonObject, JsonValue};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::System::WinRT::IBufferByteAccess;

/// `void (*)(const wchar_t* result)` — the JSON result (NUL-terminated), or
/// NULL on error/failure.
type UwpOcrCallback = Option<unsafe extern "system" fn(*const u16)>;

/// Per-recognition state (the C++ `UwpOcr`). Boxed and handed to the caller
/// as an opaque pointer.
struct UwpOcr {
    engine: OcrEngine,
    callback: UwpOcrCallback,
}

/// Assert `Send` for the WinRT objects moved onto the recognition thread —
/// WinRT OCR objects are agile, exactly as the C++ relies on when it
/// resumes the coroutine on a background thread.
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
    let slice = unsafe { core::slice::from_raw_parts(p, len) };
    HSTRING::from_wide(slice).unwrap_or_default()
}

unsafe fn fire(cb: UwpOcrCallback, ptr: *const u16) {
    if let Some(f) = cb {
        unsafe { f(ptr) };
    }
}

/// Create an OCR engine for `language` and register `callback`. Returns an
/// opaque handle, or NULL if OCR is unavailable for that language.
///
/// # Safety
/// `language` must be NULL or a valid NUL-terminated wide string. `callback`
/// must remain valid until the handle is terminated.
#[no_mangle]
pub unsafe extern "system" fn uwpOcr_initialize(
    language: *const u16,
    callback: UwpOcrCallback,
) -> *mut c_void {
    let tag = unsafe { wide_to_hstring(language) };
    let lang = match Language::CreateLanguage(&tag) {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };
    let engine = match OcrEngine::TryCreateFromLanguage(&lang) {
        Ok(e) if !e.as_raw().is_null() => e,
        _ => return core::ptr::null_mut(),
    };
    Box::into_raw(Box::new(UwpOcr { engine, callback })) as *mut c_void
}

/// Destroy a handle from [`uwpOcr_initialize`].
///
/// # Safety
/// `instance` must be NULL or a handle from `uwpOcr_initialize`.
#[no_mangle]
pub unsafe extern "system" fn uwpOcr_terminate(instance: *mut c_void) {
    if !instance.is_null() {
        drop(unsafe { Box::from_raw(instance as *mut UwpOcr) });
    }
}

/// Recognise a BGRA8 image (`width` x `height`, `RGBQUAD` rows). Returns
/// immediately; the result is delivered via the callback on a background
/// thread.
///
/// # Safety
/// `instance` must be a valid handle; `image` must point to at least
/// `4 * width * height` bytes.
#[no_mangle]
pub unsafe extern "system" fn uwpOcr_recognize(
    instance: *mut c_void,
    image: *const u8,
    width: u32,
    height: u32,
) {
    if instance.is_null() || image.is_null() {
        return;
    }
    let ocr = unsafe { &*(instance as *const UwpOcr) };
    let callback = ocr.callback;

    let bitmap = match unsafe { build_bitmap(image, width, height) } {
        Ok(b) => b,
        Err(_) => {
            unsafe { fire(callback, core::ptr::null()) };
            return;
        }
    };

    // Move the engine + bitmap onto a background thread (WinRT agile), run
    // the async recognise there, and fire the callback with the JSON.
    let payload = SendWrap((ocr.engine.clone(), bitmap));
    std::thread::spawn(move || {
        let SendWrap((engine, bitmap)) = payload;
        match recognize_to_json(&engine, &bitmap) {
            Ok(hs) => {
                let mut wide = hs.as_wide().to_vec();
                wide.push(0);
                unsafe { fire(callback, wide.as_ptr()) };
            }
            Err(_) => unsafe { fire(callback, core::ptr::null()) },
        }
    });
}

/// Return the available OCR language tags as a `;`-terminated `BSTR`
/// (e.g. `"de-de;en-us;"`). The caller frees it (`SysFreeString`).
///
/// # Safety
/// Returns an owned `BSTR`; the caller must free it with `SysFreeString`.
#[no_mangle]
pub unsafe extern "system" fn uwpOcr_getLanguages() -> *mut u16 {
    let mut s = String::new();
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() {
        for lang in langs {
            if let Ok(tag) = lang.LanguageTag() {
                s.push_str(&tag.to_string_lossy());
                s.push(';');
            }
        }
    }
    let wide: Vec<u16> = s.encode_utf16().collect();
    BSTR::from_wide(&wide).unwrap_or_default().into_raw() as *mut u16
}

/// Copy the BGRA image into a WinRT `SoftwareBitmap`.
unsafe fn build_bitmap(
    image: *const u8,
    width: u32,
    height: u32,
) -> windows::core::Result<SoftwareBitmap> {
    let num_bytes = 4u32.saturating_mul(width).saturating_mul(height);
    let buf = Buffer::Create(num_bytes)?;
    let bba: IBufferByteAccess = buf.cast()?;
    let dst = unsafe { bba.Buffer()? };
    unsafe {
        core::ptr::copy_nonoverlapping(image, dst, num_bytes as usize);
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

/// Run recognition and build the same JSON the C++ produced: an array of
/// lines, each an array of words `{x, y, width, height, text}`.
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
