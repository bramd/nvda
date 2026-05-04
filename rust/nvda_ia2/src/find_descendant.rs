//! Port of `findContentDescendant` from
//! `nvdaHelper/remote/IA2Support.cpp:229-312`. Recursive IA2 hypertext
//! walk that locates a content descendant for caret / selection / first
//! / last navigation.

use crate::interfaces::{IAccessible2, IAccessibleHypertext, IAccessibleText};
use windows::core::{Interface, VARIANT};
use windows::Win32::UI::Accessibility::IAccessible;

/// Discriminant for the `what` parameter. Pre-filtered by the C++ caller
/// to one of these five values; out-of-range tags yield `false` from the
/// shim.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindWhat {
    First = 0,
    Caret = 1,
    Last = 2,
    SelectionStart = 3,
    SelectionEnd = 4,
}

impl FindWhat {
    fn from_raw(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::First),
            1 => Some(Self::Caret),
            2 => Some(Self::Last),
            3 => Some(Self::SelectionStart),
            4 => Some(Self::SelectionEnd),
            _ => None,
        }
    }

    /// `Last` and `SelectionEnd` iterate children in reverse order.
    fn is_reverse(&self) -> bool {
        matches!(self, Self::Last | Self::SelectionEnd)
    }
}

/// C-callable replacement for `findContentDescendant`.
///
/// On `true`, both `descendant_id` and `descendant_offset` are written.
/// On `false`, neither is written -- the C++ caller is expected to read
/// them only on `true`, matching the original contract.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `descendant_id` and `descendant_offset` must be valid writable
///   `int*` pointers on success; null is rejected up front.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_find_content_descendant(
    pacc2: *mut core::ffi::c_void,
    what: u32,
    descendant_id: *mut i32,
    descendant_offset: *mut i32,
) -> bool {
    if pacc2.is_null() || descendant_id.is_null() || descendant_offset.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let what = match FindWhat::from_raw(what) {
        Some(w) => w,
        None => return false,
    };
    match unsafe { find_content_descendant(acc2, what) } {
        Some((id, off)) => {
            unsafe {
                *descendant_id = id;
                *descendant_offset = off;
            }
            true
        }
        None => false,
    }
}

/// Pure-Rust port of `findContentDescendant`. Returns `Some((id, offset))`
/// when a content descendant is found, `None` otherwise.
unsafe fn find_content_descendant(
    pacc2: &IAccessible2,
    what: FindWhat,
) -> Option<(i32, i32)> {
    // If this node is text-bearing, work the offset path.
    if let Ok(text) = pacc2.cast::<IAccessibleText>() {
        let offset: i32 = match what {
            FindWhat::First => 0,
            FindWhat::Caret => unsafe { text.get_caretOffset() }.ok()?,
            FindWhat::Last => {
                let n = unsafe { text.get_nCharacters() }.unwrap_or(0);
                if n > 0 { n - 1 } else { 0 }
            }
            FindWhat::SelectionStart | FindWhat::SelectionEnd => {
                let n = unsafe { text.get_nSelections() }.unwrap_or(0);
                if n == 0 {
                    return None;
                }
                let (start, end) = unsafe { text.get_selection(0) }.ok()?;
                if matches!(what, FindWhat::SelectionStart) { start } else { end - 1 }
            }
        };

        // If this offset lands on an embedded hyperlink, recurse into the
        // hyperlinked child.
        if let Ok(hyper) = pacc2.cast::<IAccessibleHypertext>() {
            let hi = unsafe { hyper.get_hyperlinkIndex(offset) }.unwrap_or(-1);
            if hi >= 0 {
                if let Ok(hyperlink) = unsafe { hyper.get_hyperlink(hi) } {
                    if let Ok(child) = hyperlink.cast::<IAccessible2>() {
                        if let Some(found) =
                            unsafe { find_content_descendant(&child, what) }
                        {
                            return Some(found);
                        }
                        // Caret fallback: if Caret didn't resolve in the
                        // child, try First inside the same child. Mirrors
                        // C++ lines 280-282.
                        if matches!(what, FindWhat::Caret) {
                            if let Some(found) = unsafe {
                                find_content_descendant(&child, FindWhat::First)
                            } {
                                return Some(found);
                            }
                        }
                    }
                }
            }
        }

        // No deeper descendant; this node is the answer.
        let id = unsafe { pacc2.get_uniqueID() }.ok()?;
        return Some((id, offset));
    }

    // Not text-bearing; iterate children. LAST / SELECTIONEND iterate
    // in reverse order.
    let pacc: &IAccessible = pacc2;
    let child_count = unsafe { pacc.accChildCount() }.unwrap_or(0);
    if child_count <= 0 {
        return None;
    }
    for i in 1..=child_count {
        let idx = if what.is_reverse() {
            child_count - (i - 1)
        } else {
            i
        };
        let varchild = VARIANT::from(idx);
        let child_disp = match unsafe { pacc.get_accChild(&varchild) } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let child_acc2: IAccessible2 = match child_disp.cast() {
            Ok(a) => a,
            Err(_) => continue,
        };
        if let Some(found) = unsafe { find_content_descendant(&child_acc2, what) } {
            return Some(found);
        }
    }
    None
}
