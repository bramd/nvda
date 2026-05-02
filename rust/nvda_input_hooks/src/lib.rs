/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

//! NVDA Helper Remote: Rust port of Windows-hook-based input event handlers.
//!
//! This crate is built as a `staticlib` and linked into `nvdaHelperRemote.dll`.
//! It is loaded into target processes via DLL injection, so the same code runs
//! inside Word, Chrome, every browser tab, etc. Keep allocations minimal and do
//! not rely on host-process global state.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutNameW;
use windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, KL_NAMELENGTH, WH_CALLWNDPROC, WM_INPUTLANGCHANGE,
};

// Symbols resolved at link time inside nvdaHelperRemote.dll.
//
// `registerWindowsHook` and `unregisterWindowsHook` are NVDA helpers
// (declared in `nvdaHelperRemote.h`) that wrap `SetWindowsHookEx` /
// `UnhookWindowsHookEx` and track installed hooks for cleanup at injection
// teardown.
//
// `nvdaControllerInternal_inputLangChangeNotify` is a MIDL-generated client
// stub; the IDL is at
// `nvdaHelper/interfaces/nvdaControllerInternal/nvdaControllerInternal.idl`.
//
// `isTSFThread()` is defined in `nvdaHelper/remote/tsf.cpp`. Declaring it
// here keeps Rust as a peer of the C++ files inside nvdaHelperRemote.dll.
// The C++ version returns `bool` which is one byte; we use `u8` for ABI
// safety and compare against 0.
extern "C" {
    fn registerWindowsHook(
        hook_type: i32,
        proc: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
    );
    fn unregisterWindowsHook(
        hook_type: i32,
        proc: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
    );
    fn nvdaControllerInternal_inputLangChangeNotify(
        thread_id: u32,
        hkl: u32,
        layout_name: *const u16,
    ) -> u32;
    fn isTSFThread() -> u8;
}

/// Last-seen LPARAM (the new HKL packed as a pointer-sized integer). Used to
/// suppress duplicate notifications when the OS posts WM_INPUTLANGCHANGE
/// twice in a row.
///
/// Storing as AtomicIsize because LPARAM is pointer-sized; we only ever read
/// and write it on the same thread (the hook runs on the host's UI thread),
/// but using an atomic gives us a clean mutable static without `unsafe`
/// at every access.
static LAST_INPUT_LANG_CHANGE: AtomicIsize = AtomicIsize::new(0);

/// CALLWNDPROC hook callback. Runs on the host process's UI thread for every
/// SendMessage routed through the message queue.
unsafe extern "system" fn input_lang_change_hook_proc(
    _code: i32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pcwp = lparam.0 as *const CWPSTRUCT;
    if pcwp.is_null() {
        return LRESULT(0);
    }
    // SAFETY: `pcwp` is a CWPSTRUCT supplied by the OS for the duration of
    // the hook callback. It is non-null per the null check above.
    let cwp = unsafe { &*pcwp };
    if cwp.message != WM_INPUTLANGCHANGE {
        return LRESULT(0);
    }
    if cwp.lParam.0 == LAST_INPUT_LANG_CHANGE.load(Ordering::Relaxed) {
        return LRESULT(0);
    }
    // SAFETY: isTSFThread is a C function that takes no arguments and is safe
    // to call from any thread.
    //
    // TSF-aware threads handle their own input-language tracking via
    // tsf.cpp; skip the notify to avoid duplicates. Note: we still update
    // LAST_INPUT_LANG_CHANGE below regardless, mirroring the C++ original
    // (the de-duplication state advances even when we don't notify).
    if unsafe { isTSFThread() } == 0 {
        // Read the current keyboard layout name (KL_NAMELENGTH is 9 wide
        // chars including the trailing NUL per MSDN).
        let mut buf = [0u16; KL_NAMELENGTH as usize];
        // SAFETY: GetKeyboardLayoutNameW writes up to KL_NAMELENGTH wide
        // chars, including the trailing NUL.
        let _ = unsafe { GetKeyboardLayoutNameW(&mut buf) };
        // SAFETY: linked at DLL-load time within nvdaHelperRemote.dll.
        unsafe {
            nvdaControllerInternal_inputLangChangeNotify(
                windows::Win32::System::Threading::GetCurrentThreadId(),
                cwp.lParam.0 as u32,
                buf.as_ptr(),
            );
        }
    }
    LAST_INPUT_LANG_CHANGE.store(cwp.lParam.0, Ordering::Relaxed);
    LRESULT(0)
}

/// Called from `inProcess.cpp` when the in-process manager thread starts up.
#[no_mangle]
pub extern "C" fn inputLangChange_inProcess_initialize() {
    unsafe { registerWindowsHook(WH_CALLWNDPROC.0, input_lang_change_hook_proc) };
}

/// Called from `inProcess.cpp` when the in-process manager thread terminates.
#[no_mangle]
pub extern "C" fn inputLangChange_inProcess_terminate() {
    unsafe { unregisterWindowsHook(WH_CALLWNDPROC.0, input_lang_change_hook_proc) };
}
