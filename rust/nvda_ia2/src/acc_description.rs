//! Port of `getAccDescription` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:324`.
//!
//! Wraps `IAccessible::get_accDescription`. Returns `Some(wide-chars)`
//! when the COM call succeeds and the BSTR is non-null (an empty
//! zero-length non-null BSTR is preserved as `Some(vec![])`), and
//! `None` when the COM call fails or returns a NULL BSTR.

use crate::bstr::is_bstr_null;
use crate::interfaces::IAccessible2;
use windows::core::VARIANT;
use windows::Win32::UI::Accessibility::IAccessible;

/// Rust-native variant of `getAccDescription` for in-crate callers.
/// Returns `Some(wide-chars)` when the COM call succeeded and produced
/// a non-NULL BSTR (the empty string is preserved as `Some(vec![])`).
/// Returns `None` on failure or NULL BSTR.
pub(crate) fn get_acc_description_native(
    acc: &IAccessible2,
    childid: i32,
) -> Option<Vec<u16>> {
    let pacc_msaa: &IAccessible = acc;
    let varchild = VARIANT::from(childid);
    let desc = match unsafe { pacc_msaa.get_accDescription(&varchild) } {
        Ok(d) => d,
        Err(_) => return None,
    };
    if is_bstr_null(&desc) {
        return None;
    }
    Some(desc.as_wide().to_vec())
}
