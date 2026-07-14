//! Benchmark-only PyO3 module comparing a representative UIA hot path —
//! "walk a container's children and batch-read a few properties each" — done
//! in **windows-rs** against the same work in **comtypes-in-Python** (see the
//! Python driver `bench_uia.py`).
//!
//! It exposes:
//! * `make_test_window(n)` — build a deterministic UIA subtree (a window with
//!   `n` labelled child controls) on a message-pumping thread, so both sides
//!   walk the *same* tree.
//! * `rust_walk(hwnd, prop_ids)` — the whole walk + property reads done in
//!   one PyO3 call (the realistic "coarse" Rust deployment: the Python↔Rust
//!   boundary is crossed once and amortized over the walk). Returns a
//!   checksum so both sides can be verified to do identical work.
//! * `rust_walk_granular(hwnd, prop_ids)` — returns per-child property values
//!   to Python (one value object per property), to expose what a naive 1:1
//!   binding would cost by materialising Python objects per read.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc;

use pyo3::prelude::*;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement,
    IUIAutomationElementArray, TreeScope_Children, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    RegisterClassW, TranslateMessage, CW_USEDEFAULT, HMENU, MSG, WNDCLASSW,
    WINDOW_EX_STYLE, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

thread_local! {
    /// Cached UIA client for the calling (Python) thread — created once and
    /// reused, mirroring how NVDA holds a single `clientObject`. Interfaces
    /// are apartment-bound, and the Python driver always calls on one thread.
    static UIA: RefCell<Option<IUIAutomation>> = const { RefCell::new(None) };
    /// A pre-built cached element array (from `build_cache`) so `read_cached`
    /// can time *only* local cached-property reads, isolated from the one
    /// marshaled fetch.
    static CACHED: RefCell<Option<IUIAutomationElementArray>> =
        const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Build a deterministic test window with `n` labelled STATIC children on a
/// dedicated message-pumping thread (so cross-thread UIA reads are serviced),
/// and return the parent window handle. The thread is detached and lives for
/// the process.
#[pyfunction]
fn make_test_window(n: usize) -> PyResult<usize> {
    let (tx, rx) = mpsc::channel::<isize>();
    std::thread::spawn(move || unsafe {
        let class_name = wide("uia_bench_window_class");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // Ignore "class already exists" if make_test_window is called twice.
        RegisterClassW(&wc);

        let title = wide("uia_bench test window");
        let parent = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            600,
            HWND::default(),
            HMENU::default(),
            HINSTANCE::default(),
            None,
        )
        .expect("CreateWindowExW parent");

        let static_class = wide("STATIC");
        for i in 0..n {
            let label = wide(&format!("Item number {i}"));
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(static_class.as_ptr()),
                PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                0,
                (i as i32) * 18,
                380,
                16,
                parent,
                HMENU::default(),
                HINSTANCE::default(),
                None,
            );
        }

        let _ = tx.send(parent.0 as isize);

        // Pump so UIA requests to this window's provider are serviced.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });

    let hwnd = rx
        .recv()
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("window thread failed"))?;
    Ok(hwnd as usize)
}

/// Best-effort COM init on the calling thread (STA, matching comtypes'
/// default). Ignores "already initialised" outcomes.
fn ensure_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Get (or lazily create) the per-thread UIA client, run `f` with it.
fn with_uia<R>(f: impl FnOnce(&IUIAutomation) -> windows::core::Result<R>) -> PyResult<R> {
    ensure_com();
    UIA.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let uia: IUIAutomation = unsafe {
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            }
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "CoCreateInstance(CUIAutomation) failed: {e}"
                ))
            })?;
            *slot = Some(uia);
        }
        f(slot.as_ref().unwrap()).map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("UIA error: {e}"))
        })
    })
}

/// A stable checksum of a UIA property VARIANT, so the Rust and Python walks
/// can be verified to read identical data. Strings contribute their length,
/// I4/int their value, BOOL 0/1; anything else 0.
unsafe fn variant_checksum(v: &windows::core::VARIANT) -> u64 {
    const VT_I4: u16 = 3;
    const VT_BSTR: u16 = 8;
    const VT_BOOL: u16 = 11;
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_BSTR {
        let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
        if p.is_null() {
            return 0;
        }
        let byte_len =
            unsafe { ((p as *const u8).sub(4) as *const u32).read_unaligned() };
        (byte_len / 2) as u64
    } else if vt == VT_I4 {
        (unsafe { raw.Anonymous.Anonymous.Anonymous.lVal } as i64) as u64
    } else if vt == VT_BOOL {
        if unsafe { raw.Anonymous.Anonymous.Anonymous.boolVal } != 0 {
            1
        } else {
            0
        }
    } else {
        0
    }
}

/// The "coarse" Rust path: `ElementFromHandle` -> children -> batch-read
/// `prop_ids` on each -> return a checksum. The whole walk is one PyO3 call.
#[pyfunction]
fn rust_walk(hwnd: usize, prop_ids: Vec<i32>) -> PyResult<u64> {
    with_uia(|uia| unsafe {
        let root = uia.ElementFromHandle(HWND(hwnd as *mut c_void))?;
        let condition = uia.CreateTrueCondition()?;
        let children = root.FindAll(TreeScope_Children, &condition)?;
        let count = children.Length()?;
        let mut checksum: u64 = 0;
        for i in 0..count {
            let child: IUIAutomationElement = children.GetElement(i)?;
            for &pid in &prop_ids {
                let val =
                    child.GetCurrentPropertyValue(UIA_PROPERTY_ID(pid))?;
                checksum = checksum.wrapping_add(variant_checksum(&val));
            }
        }
        Ok(checksum)
    })
}

/// The "granular" path: return each child's property values to Python as a
/// list of lists, materialising a Python object per property — to expose the
/// per-read boundary + object-creation cost a naive 1:1 binding would incur.
#[pyfunction]
fn rust_walk_granular(
    py: Python<'_>,
    hwnd: usize,
    prop_ids: Vec<i32>,
) -> PyResult<u64> {
    // Collect raw checksums but cross the boundary once per property value by
    // returning them via a Python list (built + discarded) to model the cost.
    let rows: Vec<Vec<u64>> = with_uia(|uia| unsafe {
        let root = uia.ElementFromHandle(HWND(hwnd as *mut c_void))?;
        let condition = uia.CreateTrueCondition()?;
        let children = root.FindAll(TreeScope_Children, &condition)?;
        let count = children.Length()?;
        let mut rows = Vec::with_capacity(count as usize);
        for i in 0..count {
            let child: IUIAutomationElement = children.GetElement(i)?;
            let mut row = Vec::with_capacity(prop_ids.len());
            for &pid in &prop_ids {
                let val =
                    child.GetCurrentPropertyValue(UIA_PROPERTY_ID(pid))?;
                row.push(variant_checksum(&val));
            }
            rows.push(row);
        }
        Ok(rows)
    })?;
    // Materialise as Python objects (the cost a 1:1 binding pays), then sum.
    let mut checksum: u64 = 0;
    for row in &rows {
        let pyrow = pyo3::types::PyList::new(py, row)?;
        for item in pyrow.iter() {
            checksum = checksum.wrapping_add(item.extract::<u64>()?);
        }
    }
    Ok(checksum)
}

/// The cached "coarse" path — NVDA's real pattern: one `FindAllBuildCache`
/// marshaled fetch of the requested properties, then **local** cached reads.
#[pyfunction]
fn rust_walk_cached(hwnd: usize, prop_ids: Vec<i32>) -> PyResult<u64> {
    with_uia(|uia| unsafe {
        let root = uia.ElementFromHandle(HWND(hwnd as *mut c_void))?;
        let condition = uia.CreateTrueCondition()?;
        let cache = uia.CreateCacheRequest()?;
        for &pid in &prop_ids {
            cache.AddProperty(UIA_PROPERTY_ID(pid))?;
        }
        let children =
            root.FindAllBuildCache(TreeScope_Children, &condition, &cache)?;
        let count = children.Length()?;
        let mut checksum: u64 = 0;
        for i in 0..count {
            let child = children.GetElement(i)?;
            for &pid in &prop_ids {
                let val = child.GetCachedPropertyValue(UIA_PROPERTY_ID(pid))?;
                checksum = checksum.wrapping_add(variant_checksum(&val));
            }
        }
        Ok(checksum)
    })
}

/// Build + stash a cached element array (one marshaled fetch), returning the
/// child count. Pairs with [`read_cached`] to isolate local-read cost.
#[pyfunction]
fn build_cache(hwnd: usize, prop_ids: Vec<i32>) -> PyResult<usize> {
    let n = with_uia(|uia| unsafe {
        let root = uia.ElementFromHandle(HWND(hwnd as *mut c_void))?;
        let condition = uia.CreateTrueCondition()?;
        let cache = uia.CreateCacheRequest()?;
        for &pid in &prop_ids {
            cache.AddProperty(UIA_PROPERTY_ID(pid))?;
        }
        let children =
            root.FindAllBuildCache(TreeScope_Children, &condition, &cache)?;
        let count = children.Length()? as usize;
        CACHED.with(|c| *c.borrow_mut() = Some(children));
        Ok(count)
    })?;
    Ok(n)
}

/// Read the pre-built cache (from [`build_cache`]) — **pure local cached
/// reads**, no marshaling — so timing this isolates the binding overhead
/// (windows-rs vtable) that comtypes' dynamic dispatch is compared against.
#[pyfunction]
fn read_cached(prop_ids: Vec<i32>) -> PyResult<u64> {
    CACHED.with(|c| {
        let slot = c.borrow();
        let children = slot.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("build_cache first")
        })?;
        unsafe {
            let count = children.Length().map_err(uia_err)?;
            let mut checksum: u64 = 0;
            for i in 0..count {
                let child = children.GetElement(i).map_err(uia_err)?;
                for &pid in &prop_ids {
                    let val = child
                        .GetCachedPropertyValue(UIA_PROPERTY_ID(pid))
                        .map_err(uia_err)?;
                    checksum = checksum.wrapping_add(variant_checksum(&val));
                }
            }
            Ok(checksum)
        }
    })
}

fn uia_err(e: windows::core::Error) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(format!("UIA error: {e}"))
}

#[pymodule]
fn uia_bench(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(make_test_window, m)?)?;
    m.add_function(wrap_pyfunction!(rust_walk, m)?)?;
    m.add_function(wrap_pyfunction!(rust_walk_granular, m)?)?;
    m.add_function(wrap_pyfunction!(rust_walk_cached, m)?)?;
    m.add_function(wrap_pyfunction!(build_cache, m)?)?;
    m.add_function(wrap_pyfunction!(read_cached, m)?)?;
    Ok(())
}
