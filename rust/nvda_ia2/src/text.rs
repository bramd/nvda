//! Port of `getTextFromIAccessible` from
//! `nvdaHelper/remote/textFromIAccessible.cpp`.
//!
//! This module exposes the pure `is_empty_text` helper and the
//! `nvda_ia2_get_text_from_iaccessible` extern C shim for C++ callers.

use crate::interfaces::{IAccessible2, IAccessibleHypertext, IAccessibleText};
use std::collections::BTreeMap;
use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::UI::Accessibility::{AccessibleChildren, IAccessible};

pub const OBJ_REPLACEMENT_CHAR: u16 = 0xFFFC;

/// Mirrors the C++ `isEmpty` helper in
/// `nvdaHelper/remote/textFromIAccessible.cpp:27`. A text run is "empty"
/// for our purposes if every character is either whitespace or the
/// embedded-object replacement character.
pub fn is_empty_text(chars: &[u16]) -> bool {
    chars.iter().all(|&c| c == OBJ_REPLACEMENT_CHAR || is_whitespace_w(c))
}

/// Mirrors the C runtime `iswspace` for the BMP characters NVDA actually
/// sees through BSTRs. The C++ code calls `iswspace` directly; we
/// implement the standard whitespace set ourselves to keep this a pure
/// Rust function (testable without the CRT).
fn is_whitespace_w(c: u16) -> bool {
    matches!(
        c,
        0x0009 // tab
        | 0x000A // line feed
        | 0x000B // vertical tab
        | 0x000C // form feed
        | 0x000D // carriage return
        | 0x0020 // space
        | 0x00A0 // no-break space (iswspace returns true for this in many locales,
                 // and NVDA encounters it from web content)
    )
}

/// `IA2_TEXT_OFFSET_LENGTH` per `include/ia2/api/IA2CommonTypes.idl:160`.
const IA2_TEXT_OFFSET_LENGTH: i32 = -1;

/// `VT_DISPATCH` per OAIDL.h. windows-core 0.58 doesn't expose a typed
/// constant we can use against the raw `imp::VARIANT` `vt` field
/// (`u16`-typed in the imp module), so we declare it locally.
const VT_DISPATCH_RAW: u16 = 9;

/// C-callable callback. Invoked once at the end of
/// [`nvda_ia2_get_text_from_iaccessible`] with the accumulated text. Mirrors
/// the C++ `textBuf.append(ptr, len)` pattern.
///
/// # Safety
///
/// The callback must not unwind. The pointer is valid for `len` `u16`
/// elements; the callback must copy the data before returning.
pub type AppendCharsCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    ptr: *const u16,
    len: usize,
);

/// C-callable replacement for `getTextFromIAccessible`.
///
/// `pacc2` is borrowed (no `Release`). `cb` is always invoked exactly once
/// with the collected text (possibly empty) before this function returns,
/// regardless of whether the return value is `true` or `false`.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `cb` must be a valid function pointer; `ctx` is opaque user data.
/// * `cb` must not unwind. The C++ adapter (`textFromIAccessible.cpp`)
///   must catch any `std::bad_alloc` from `std::wstring::append` (or accept
///   process termination on OOM) before returning to Rust.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_text_from_iaccessible(
    pacc2: *mut core::ffi::c_void,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
    ctx: *mut core::ffi::c_void,
    cb: AppendCharsCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let mut buf: Vec<u16> = Vec::new();
    let got_text = get_text_from_iaccessible_collect(
        &mut buf,
        acc2,
        use_new_text,
        recurse,
        include_top_level_text,
    );
    cb(ctx, buf.as_ptr(), buf.len());
    got_text
}

/// Pure-Rust port of `getTextFromIAccessible`.
pub(crate) fn get_text_from_iaccessible_collect(
    text_buf: &mut Vec<u16>,
    pacc2: &IAccessible2,
    use_new_text: bool,
    recurse: bool,
    include_top_level_text: bool,
) -> bool {
    let mut got_text = false;
    let pacc_text: Option<IAccessibleText> = pacc2.cast().ok();

    if pacc_text.is_none() && recurse && !use_new_text {
        // No IAccessibleText interface, so try children instead. Mirrors
        // textFromIAccessible.cpp:79-104.
        let pacc: &IAccessible = pacc2; // Deref to the IAccessible base.
        let child_count = match unsafe { pacc.accChildCount() } {
            Ok(n) if n > 0 => n,
            _ => return got_text,
        };
        let mut variants: Vec<VARIANT> = vec![VARIANT::default(); child_count as usize];
        let mut filled: i32 = 0;
        if unsafe { AccessibleChildren(pacc, 0, &mut variants[..], &mut filled) }.is_err() {
            return got_text;
        }
        variants.truncate(filled as usize);
        for v in variants.iter() {
            // VT_DISPATCH child contains an IDispatch we QI to IAccessible2.
            let pdisp = variant_dispatch_ptr(v);
            if pdisp.is_null() {
                continue;
            }
            // Borrow the IDispatch -- the VARIANT owns the reference; we
            // borrow it only long enough to QI/cast. cast() returns a fresh
            // owned IAccessible2 with its own AddRef.
            let disp: &IDispatch = match unsafe { IDispatch::from_raw_borrowed(&pdisp) } {
                Some(d) => d,
                None => continue,
            };
            let pacc2_child: IAccessible2 = match disp.cast() {
                Ok(a) => a,
                Err(_) => continue,
            };
            if child_is_live_off(&pacc2_child) {
                continue;
            }
            got_text |= get_text_from_iaccessible_collect(
                text_buf,
                &pacc2_child,
                false, // use_new_text
                true,  // recurse
                true,  // include_top_level_text
            );
        }
    } else if let Some(pacc_text) = pacc_text.as_ref() {
        // We can use IAccessibleText. Mirrors textFromIAccessible.cpp:105-160.
        // We hold the BSTR alive for the whole loop so as_wide()'s slice
        // stays valid; iterate by index because we need start_offset + idx.
        let (bstr_text, start_offset): (Option<BSTR>, i32) = if use_new_text {
            match unsafe { pacc_text.get_newText() } {
                Ok(mut seg) if !is_bstr_null(&seg.text) => {
                    let start = seg.start;
                    // Move the BSTR out of the segment so we own it independently.
                    let text = core::mem::take(&mut seg.text);
                    (Some(text), start)
                }
                _ => (None, 0),
            }
        } else {
            match unsafe { pacc_text.get_text(0, IA2_TEXT_OFFSET_LENGTH) } {
                Ok(b) if !is_bstr_null(&b) => (Some(b), 0),
                _ => (None, 0),
            }
        };
        if let Some(bstr_text) = bstr_text {
            let chars = bstr_text.as_wide();
            let pacc_hyper: Option<IAccessibleHypertext> = if recurse {
                pacc2.cast().ok()
            } else {
                None
            };
            for (idx, &real_char) in chars.iter().enumerate() {
                let mut char_added = false;
                if real_char == OBJ_REPLACEMENT_CHAR {
                    if let Some(pacc_hyper) = pacc_hyper.as_ref() {
                        let char_index = start_offset + idx as i32;
                        // `get_hyperlinkIndex` returns `Ok(-1)` for the
                        // S_FALSE "not on a link" contract; guard with
                        // `>= 0` to skip a wasted `get_hyperlink` call.
                        let hyperlink_index = unsafe {
                            pacc_hyper.get_hyperlinkIndex(char_index)
                        }
                        .ok()
                        .filter(|&i| i >= 0);
                        if let Some(hyperlink_index) = hyperlink_index {
                            if let Ok(pacc_hyperlink) =
                                unsafe { pacc_hyper.get_hyperlink(hyperlink_index) }
                            {
                                if let Ok(pacc2_child) =
                                    pacc_hyperlink.cast::<IAccessible2>()
                                {
                                    if !child_is_live_off(&pacc2_child)
                                        && get_text_from_iaccessible_collect(
                                            text_buf, &pacc2_child, false, true, true,
                                        )
                                    {
                                        got_text = true;
                                    }
                                    char_added = true;
                                }
                            }
                        }
                    }
                }
                if !char_added && include_top_level_text {
                    text_buf.push(real_char);
                    if real_char != OBJ_REPLACEMENT_CHAR && !is_whitespace_w(real_char) {
                        got_text = true;
                    }
                }
            }
            text_buf.push(b' ' as u16);
            // bstr_text drops here, freeing the BSTR.
        }
    }

    if !got_text && !use_new_text {
        // Fall back to name and/or description. Mirrors
        // textFromIAccessible.cpp:162-165.
        got_text = append_name_description(text_buf, pacc2);
    }
    got_text
}

/// Mirrors `appendNameDescription` in `textFromIAccessible.cpp:39`.
fn append_name_description(text_buf: &mut Vec<u16>, pacc2: &IAccessible2) -> bool {
    let pacc: &IAccessible = pacc2;
    let varchild = VARIANT::from(0i32); // CHILDID_SELF
    let mut got_text = false;

    if let Ok(name) = unsafe { pacc.get_accName(&varchild) } {
        let chars = name.as_wide();
        if !is_empty_text(chars) {
            text_buf.extend_from_slice(chars);
            text_buf.push(b' ' as u16);
            got_text = true;
        }
    }
    if let Ok(desc) = unsafe { pacc.get_accDescription(&varchild) } {
        let chars = desc.as_wide();
        if !is_empty_text(chars) {
            text_buf.extend_from_slice(chars);
            got_text = true;
        }
    }
    got_text
}

/// Returns true if the `live` IA2 attribute equals `"off"` for `pacc2`.
/// Mirrors the live-region filter at `textFromIAccessible.cpp:90` and
/// `:140`. A failed `get_attributes` (no attributes string, or HRESULT
/// error) does not suppress the child -- this matches the C++ behavior,
/// where an absent `live` key falls through to the recursion/append branch.
fn child_is_live_off(pacc2: &IAccessible2) -> bool {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(bstr) = unsafe { pacc2.get_attributes() } {
        if !is_bstr_null(&bstr) {
            map = crate::attribs::parse_attribs(&bstr.to_string());
        }
    }
    matches!(map.get("live"), Some(v) if v == "off")
}

/// Extract the `pdispVal` raw pointer from a `VARIANT` if its `vt` is
/// `VT_DISPATCH`. Returns `null` for any other VARENUM (including
/// `VT_I4`, which `AccessibleChildren` may also produce). The pointer
/// is non-owning -- the VARIANT retains the AddRef.
///
/// SAFETY: `windows_core::imp::VARIANT` is the same `VARIANT` C structure
/// from `OAIDL.h`. Reading `vt` is always safe (it's a discriminant); we
/// only read `pdispVal` after confirming `vt == VT_DISPATCH`, which is the
/// VARENUM contract for that union member being active.
fn variant_dispatch_ptr(v: &VARIANT) -> *mut core::ffi::c_void {
    let raw = v.as_raw();
    let inner = unsafe { &raw.Anonymous.Anonymous };
    if inner.vt != VT_DISPATCH_RAW {
        return core::ptr::null_mut();
    }
    unsafe { inner.Anonymous.pdispVal }
}

/// `BSTR::is_empty()` returns true for both NULL and zero-length BSTRs.
/// We need to distinguish: a zero-length BSTR is treated as "got the call
/// back, but no text" (still trigger the trailing-space append at the end
/// of the text branch) while a NULL BSTR is "the call returned nothing
/// usable" (skip the branch entirely). Mirrors the trick used in
/// `fetch.rs`. SAFETY: `windows::core::BSTR` is `#[repr(transparent)]`
/// over a single `*const u16` field; verified in
/// `windows-strings-0.1.0/src/bstr.rs:6`.
fn is_bstr_null(bstr: &BSTR) -> bool {
    let raw_ptr: *const u16 = unsafe { *(bstr as *const _ as *const *const u16) };
    raw_ptr.is_null()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slice_is_empty() {
        assert!(is_empty_text(&[]));
    }

    #[test]
    fn all_spaces_is_empty() {
        let chars: Vec<u16> = "    ".encode_utf16().collect();
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn all_object_replacement_is_empty() {
        assert!(is_empty_text(&[OBJ_REPLACEMENT_CHAR; 5]));
    }

    #[test]
    fn mixed_spaces_and_object_replacement_is_empty() {
        let mut chars: Vec<u16> = " ".encode_utf16().collect();
        chars.push(OBJ_REPLACEMENT_CHAR);
        chars.extend("\t\n".encode_utf16());
        chars.push(OBJ_REPLACEMENT_CHAR);
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn single_letter_is_not_empty() {
        let chars: Vec<u16> = "a".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn whitespace_around_letter_is_not_empty() {
        let chars: Vec<u16> = "  a  ".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn nbsp_alone_is_empty() {
        assert!(is_empty_text(&[0x00A0]));
    }
}
