//! Port of `getChildCount` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:308`.
//!
//! Returns `accChildCount` for the given accessible, or `0` when the
//! caller has determined the element is `aria-hidden` (in which case
//! we don't recurse into its children at all).

use crate::interfaces::IAccessible2;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::IAccessible;

/// Rust-native variant of `getChildCount` for in-crate callers.
/// Returns the IA2/MSAA child count, or `0` when `is_aria_hidden` is
/// true or the COM call fails.
pub(crate) fn get_child_count_native(
    acc: &IAccessible2,
    is_aria_hidden: bool,
) -> i32 {
    if is_aria_hidden {
        return 0;
    }
    let pacc_msaa: &IAccessible = acc;
    unsafe { pacc_msaa.accChildCount() }.unwrap_or(0)
}

/// C-callable replacement for `getChildCount`. Returns the IA2/MSAA
/// child count, or `0` when `is_aria_hidden` is true. Matches the C++
/// behavior of returning `0` when `accChildCount` itself fails.
///
/// # Safety
///
/// `pacc` must be a valid `IAccessible2*` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_child_count(
    pacc: *mut core::ffi::c_void,
    is_aria_hidden: bool,
) -> i32 {
    if is_aria_hidden || pacc.is_null() {
        return 0;
    }
    let acc: &IAccessible2 =
        match IAccessible2::from_raw_borrowed(&pacc) {
            Some(a) => a,
            None => return 0,
        };
    let pacc_msaa: &IAccessible = acc;
    unsafe { pacc_msaa.accChildCount() }.unwrap_or(0)
}
