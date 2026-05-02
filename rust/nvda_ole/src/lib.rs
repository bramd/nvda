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
/// SAFETY: caller must ensure `addr` is a valid IUnknown pointer for the
/// duration of this call. The caller's pointer is NOT consumed; we AddRef via
/// `clone()` so the returned owned `IUnknown` drops cleanly without affecting
/// the caller's lifetime.
unsafe fn borrow_iunknown(addr: usize) -> Option<IUnknown> {
    if addr == 0 {
        return None;
    }
    let raw = addr as *mut std::ffi::c_void;
    let iface = IUnknown::from_raw_borrowed(&raw)?;
    Some(iface.clone())
}

pub fn get_clipboard_text(unknown_addr: usize) -> OleResult {
    let unknown = match unsafe { borrow_iunknown(unknown_addr) } {
        Some(u) => u,
        None => {
            log::warn!("getOleClipboardText: pUnknown is null.");
            return Err(E_INVALIDARG.0);
        }
    };
    let data_object: IDataObject = unknown.cast().map_err(|_| {
        log::warn!("getOleClipboardText: could not get IDataObject interface from pUnknown");
        E_NOINTERFACE.0
    })?;

    let format = FORMATETC {
        cfFormat: CF_UNICODETEXT.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };

    let medium: STGMEDIUM = unsafe { data_object.GetData(&format) }.map_err(|e| {
        log::warn!("getOleClipboardText: IDataObject::GetData failed with HRESULT 0x{:08x}", e.code().0 as u32);
        e.code().0
    })?;

    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        log::warn!("getOleClipboardText: got back invalid medium (tymed = {})", medium.tymed);
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let hglobal = unsafe { medium.u.hGlobal };
    if hglobal.is_invalid() {
        log::warn!("getOleClipboardText: medium.hGlobal is invalid");
        unsafe { ReleaseStgMedium(&mut { medium }) };
        return Err(E_FAIL.0);
    }

    let text = unsafe {
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            // Deliberate divergence from C++: oleUtils.cpp returned S_OK with
            // an empty BSTR when GlobalLock failed. We surface the failure as
            // an HRESULT so the Python caller can fall through cleanly.
            log::warn!("getOleClipboardText: GlobalLock returned null");
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

pub fn get_user_type(unknown_addr: usize, flags: u32) -> OleResult {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Ole::USERCLASSTYPE;

    let unknown = match unsafe { borrow_iunknown(unknown_addr) } {
        Some(u) => u,
        None => {
            log::warn!("getOleUserType: pUnknown is null.");
            return Err(E_INVALIDARG.0);
        }
    };
    let ole_object: IOleObject = unknown.cast().map_err(|_| {
        log::warn!("getOleUserType: could not get IOleObject interface from pUnknown");
        E_NOINTERFACE.0
    })?;

    let ole_str = unsafe { ole_object.GetUserType(USERCLASSTYPE(flags as i32)) }.map_err(|e| {
        log::warn!(
            "getOleUserType: IOleObject::GetUserType failed with HRESULT 0x{:08x} for flags {}",
            e.code().0 as u32,
            flags,
        );
        e.code().0
    })?;
    if ole_str.is_null() {
        log::warn!("getOleUserType: IOleObject::GetUserType returned null string for flags {}", flags);
        return Err(E_FAIL.0);
    }

    // ole_str is a PWSTR allocated with CoTaskMemAlloc; we own it
    let result = unsafe {
        let mut len = 0usize;
        while *ole_str.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ole_str.0, len);
        let s = String::from_utf16_lossy(slice);
        CoTaskMemFree(Some(ole_str.0 as *const _));
        s
    };

    Ok(result)
}
