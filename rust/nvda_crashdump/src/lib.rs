/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited
*/

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpNormal, MiniDumpWriteDump, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

/// Write a minidump for the current process to `path`.
///
/// `exception_pointers_addr` is the integer value of an `EXCEPTION_POINTERS*`
/// (typically from an UnhandledExceptionFilter callback). Pass 0 if no
/// exception context is available — the resulting dump is smaller but still
/// valid.
///
/// Returns true on success.
pub fn write_crash_dump(path: &str, exception_pointers_addr: usize) -> bool {
    // Encode path as UTF-16 with null terminator
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);

    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let handle = match handle {
        Ok(h) if !h.is_invalid() => h,
        _ => return false,
    };

    let mdei = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: exception_pointers_addr as *mut EXCEPTION_POINTERS,
        ClientPointers: false.into(),
    };
    let mdei_param = if exception_pointers_addr != 0 {
        Some(&mdei as *const _)
    } else {
        None
    };

    let result = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            handle,
            MiniDumpNormal,
            mdei_param,
            None,
            None,
        )
    };

    let _ = unsafe { CloseHandle(handle) };
    let _ = mdei; // keep alive until after MiniDumpWriteDump returns
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_write_dump_creates_minidump_file() {
        let tmp = std::env::temp_dir().join(format!(
            "nvda_test_dump_{}.dmp",
            std::process::id()
        ));
        let path = tmp.to_string_lossy().to_string();
        let _ = fs::remove_file(&tmp);

        // exception_pointers = 0 (NULL) — valid; produces a dump without exception info
        let ok = write_crash_dump(&path, 0);

        assert!(ok, "write_crash_dump returned false");
        let bytes = fs::read(&tmp).expect("dump file not created");
        assert!(bytes.len() > 32, "dump file is suspiciously small");
        // MINIDUMP_HEADER.Signature = 'MDMP' (0x504D444D LE)
        assert_eq!(&bytes[0..4], b"MDMP", "minidump magic missing");
        let _ = fs::remove_file(&tmp);
    }
}
