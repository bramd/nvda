//! Port of `GeckoVBufBackend_t::versionSpecificInit` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:197`.
//!
//! Walks IAccessible2 → IServiceProvider → IAccessibleApplication →
//! `get_toolkitName` and returns the resulting wide string. The gecko
//! backend state caches the result for the `is_chrome` check used
//! throughout the render.

use windows::core::Interface;
use windows::Win32::System::Com::IServiceProvider;

use crate::interfaces::{IAccessible2, IAccessibleApplication};

/// Rust-native variant for in-crate callers. Returns the toolkit
/// name as an owned `Vec<u16>`, or an empty `Vec` on COM failure.
pub(crate) fn get_toolkit_name_native(acc: &IAccessible2) -> Vec<u16> {
    let serv: IServiceProvider = match acc.cast() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let app: IAccessibleApplication = match unsafe {
        serv.QueryService(&IAccessibleApplication::IID)
    } {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    match unsafe { app.get_toolkitName() } {
        Ok(name) => name.as_wide().to_vec(),
        Err(_) => Vec::new(),
    }
}
