//! Port of `getChildCount` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:308`.
//!
//! Returns `accChildCount` for the given accessible, or `0` when the
//! caller has determined the element is `aria-hidden` (in which case
//! we don't recurse into its children at all).

use crate::interfaces::IAccessible2;
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
