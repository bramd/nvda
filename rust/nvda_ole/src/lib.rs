/*
A part of NonVisual Desktop Access (NVDA)
Copyright (C) 2026 NV Access Limited
This file may be used under the terms of the GNU General Public License,
version 2 or later, as modified by the NVDA license. For full terms and any
additional permissions, see the NVDA license file:
https://github.com/nvaccess/nvda/blob/master/copying.txt
*/

use windows::core::Interface;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOINTERFACE};
use windows::Win32::System::Com::{
    IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{IOleObject, ReleaseStgMedium, CF_UNICODETEXT};
use windows_core::IUnknown;

/// Result type used by both functions: `Ok(text)` on success, `Err(hresult)`
/// where the inner `i32` is the raw HRESULT for the Python caller to map to
/// a `WindowsError`/`OSError`.
pub type OleResult = Result<String, i32>;

/// Borrow an IUnknown from a raw pointer address (e.g. passed from Python via
/// `ctypes.cast(comObj, ctypes.c_void_p).value`). Returns `None` for null.
///
/// SAFETY: caller must ensure `addr` is a valid IUnknown pointer that outlives
/// the returned reference. We do NOT take ownership (no AddRef/Release).
unsafe fn borrow_iunknown(addr: usize) -> Option<IUnknown> {
    if addr == 0 {
        return None;
    }
    let raw = addr as *mut std::ffi::c_void;
    let iface = IUnknown::from_raw_borrowed(&raw)?;
    Some(iface.clone())
}

pub fn get_clipboard_text(unknown_addr: usize) -> OleResult {
    let unknown = unsafe { borrow_iunknown(unknown_addr) }.ok_or(E_INVALIDARG.0)?;
    let data_object: IDataObject = unknown.cast().map_err(|_| E_NOINTERFACE.0)?;

    let format = FORMATETC {
        cfFormat: CF_UNICODETEXT.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };

    let medium: STGMEDIUM = unsafe { data_object.GetData(&format) }.map_err(|e| e.code().0)?;

    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let hglobal = unsafe { medium.u.hGlobal };
    if hglobal.is_invalid() {
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let text = unsafe {
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            ReleaseStgMedium(&mut { medium });
            return Err(E_FAIL.0);
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let s = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(hglobal);
        s
    };

    unsafe { ReleaseStgMedium(&mut { medium }) };
    Ok(text)
}
