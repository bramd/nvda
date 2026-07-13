//! Rust port of `MshtmlVBufBackend_t::fillVBuf` + its helper functions
//! from `nvdaHelper/vbufBackends/mshtml/mshtml.cpp`.
//!
//! This is **Phase A**: an initial *static* render only. It renders a
//! Trident (MSHTML) DOM subtree (`IHTMLDOMNode` / `IHTMLElement` /
//! `IAccessible` …) into a Rust `nvda_vbuf::storage::Buffer` via the
//! [`VbufBuffer`] wrapper. Structure mirrors the C++ line-for-line; see
//! the referenced line numbers throughout.
//!
//! Deliberately NOT ported (left for Phase B): the custom node's COM
//! state and change sinks (`pHTMLDOMNode`, `propChangeSink`, `loadSink`,
//! `pMarkupContainer2`, `pHTMLChangeSink`), the ARIA live-region logic
//! (`preProcessLiveRegion` / `postProcessLiveRegion` / `reportLive*`),
//! and the cross-render reuse lookup (`oldNode`,
//! `getControlFieldNodeWithIdentifier` for reuse, `inNewSubtree`,
//! `atomicNodes`). The `oldNode`/`inNewSubtree`/`atomicNodes` params are
//! dropped from this signature accordingly.
//!
//! Two faithful substitutions for machinery that lives elsewhere in the
//! C++:
//! * `language` is threaded down as `inherited_language` and emitted as a
//!   normal `"language"` attribute on every control node — replacing the
//!   C++ `generateAttributesForMarkupOpeningTag` override (node.cpp) that
//!   appended `language="..."` to the opening tag.
//! * The text-node style helper reads the parent element's `currentStyle`
//!   via `IHTMLDOMNode::get_parentNode` + QI, rather than through the
//!   dropped `parentNode->pHTMLDOMNode` field (the parent DOM node is the
//!   #text node's DOM parent, so they are identical).

// Phase A ships the render logic but not the backend adapter that calls
// it, so every item here is unreachable within the crate until that lands.
#![allow(dead_code)]

use core::cell::{Cell, RefCell};
use core::mem::ManuallyDrop;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use windows::core::{Interface, BSTR, GUID, VARIANT};
use windows::Win32::System::Com::IServiceProvider;
use windows::Win32::UI::Accessibility::IAccessible;

use nvda_vbuf::{VbufBuffer, VbufControlFieldNode, VbufFieldNode};

use crate::interfaces::{
    IHTMLAttributeCollection2, IHTMLCurrentStyle, IHTMLDOMChildrenCollection,
    IHTMLDOMNode, IHTMLDOMTextNode, IHTMLElement, IHTMLElement2, IHTMLElement3,
    IHTMLUniqueName,
};

// --- Constants ------------------------------------------------------------

// MSAA roles (oleacc.h).
const ROLE_SYSTEM_MENUITEM: i32 = 0x0c;
const ROLE_SYSTEM_APPLICATION: i32 = 0x0e;
const ROLE_SYSTEM_DIALOG: i32 = 0x12;
const ROLE_SYSTEM_SEPARATOR: i32 = 0x15;
const ROLE_SYSTEM_LINK: i32 = 0x1e;
const ROLE_SYSTEM_LIST: i32 = 0x21;
const ROLE_SYSTEM_OUTLINE: i32 = 0x23;
const ROLE_SYSTEM_PAGETAB: i32 = 0x25;
const ROLE_SYSTEM_GRAPHIC: i32 = 0x28;
const ROLE_SYSTEM_STATICTEXT: i32 = 0x29;
const ROLE_SYSTEM_TEXT: i32 = 0x2a;
const ROLE_SYSTEM_PUSHBUTTON: i32 = 0x2b;
const ROLE_SYSTEM_CHECKBUTTON: i32 = 0x2c;
const ROLE_SYSTEM_RADIOBUTTON: i32 = 0x2d;
const ROLE_SYSTEM_PROGRESSBAR: i32 = 0x30;
const ROLE_SYSTEM_SLIDER: i32 = 0x33;

// MSAA states (oleacc.h).
const STATE_SYSTEM_READONLY: i32 = 0x0000_0040;
const STATE_SYSTEM_FOCUSABLE: i32 = 0x0010_0000;
const STATE_SYSTEM_LINKED: i32 = 0x0040_0000;
const STATE_SYSTEM_PROTECTED: i32 = 0x2000_0000;

// VARIANT vt values (OAIDL.h).
const VT_I2: u16 = 2;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;

// mshtml.cpp formatState bits.
const FORMATSTATE_INSERTED: u32 = 1;
const FORMATSTATE_DELETED: u32 = 2;
const FORMATSTATE_MARKED: u32 = 4;
const FORMATSTATE_STRONG: u32 = 8;
const FORMATSTATE_EMPH: u32 = 16;

// mshtml.cpp table-header types.
const TABLEHEADER_COLUMN: i32 = 0x1;
const TABLEHEADER_ROW: i32 = 0x2;

/// Per-render context, built by the backend adapter once per `render()`.
pub struct FillVBufCtx {
    /// The Win32 identifier pair's doc handle (constant per document).
    pub doc_handle: i32,
    /// The backend's `rootID` — used for the `ID == this->rootID`
    /// `isRoot` check.
    pub root_id: i32,
}

// --- Small helpers --------------------------------------------------------

/// UTF-16 for a string literal (attribute names/values).
#[inline]
pub(crate) fn u(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// UTF-16 for a signed integer's decimal representation.
#[inline]
fn ui(n: i32) -> Vec<u16> {
    u(&n.to_string())
}

/// `true` when `a` equals the ASCII string `b` (exact, case-sensitive).
#[inline]
fn weq(a: &[u16], b: &str) -> bool {
    a.iter().copied().eq(b.encode_utf16())
}

/// Case-insensitive (ASCII) comparison of a UTF-16 slice against an ASCII
/// string, mirroring the C++ `_wcsicmp(x, L"literal") == 0` idiom.
fn eq_ic(a: &[u16], b: &str) -> bool {
    let bl = b.as_bytes();
    if a.len() != bl.len() {
        return false;
    }
    a.iter().zip(bl.iter()).all(|(&c, &d)| {
        let lc = if (b'A' as u16..=b'Z' as u16).contains(&c) {
            c + 32
        } else {
            c
        };
        let ld = if d.is_ascii_uppercase() { d + 32 } else { d };
        lc == ld as u16
    })
}

/// Port of C++ `iswspace` (as used per-`wchar_t`): treat lone surrogates
/// as non-space, otherwise defer to Unicode `char::is_whitespace`.
#[inline]
fn is_wspace(c: u16) -> bool {
    match char::from_u32(c as u32) {
        Some(ch) => ch.is_whitespace(),
        None => false,
    }
}

/// Read a VARIANT's `VT_I4` value, or `None`.
#[inline]
fn variant_i4(v: &VARIANT) -> Option<i32> {
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_I4 {
        Some(unsafe { raw.Anonymous.Anonymous.Anonymous.lVal })
    } else {
        None
    }
}

/// Read a VARIANT's `VT_BSTR` value as UTF-16, or `None` for any other
/// type / a null pointer. Borrows the BSTR without taking ownership (the
/// VARIANT still owns and clears it).
fn variant_bstr(v: &VARIANT) -> Option<Vec<u16>> {
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt != VT_BSTR {
        return None;
    }
    let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
    if p.is_null() {
        return None;
    }
    let b = ManuallyDrop::new(unsafe { BSTR::from_raw(p) });
    Some(b.as_wide().to_vec())
}

/// A MSAA string getter result → UTF-16 (empty on failure). Mirrors the
/// C++ `wstring` that stays empty when the `IAccessible` getter fails
/// (and appends the value, possibly empty, when it succeeds).
fn acc_str(r: windows::core::Result<BSTR>) -> Vec<u16> {
    r.ok().map(|b| b.as_wide().to_vec()).unwrap_or_default()
}

/// Port of C++ template `queryService`: QI `from` to `IServiceProvider`,
/// then `QueryService(service, riid=T::IID)`.
pub(crate) unsafe fn query_service<T: Interface>(
    from: &impl Interface,
    service: &GUID,
) -> Option<T> {
    let sp: IServiceProvider = from.cast().ok()?;
    unsafe { sp.QueryService::<T>(service) }.ok()
}

/// Find the first occurrence of `needle` in `hay`.
fn find_subslice(hay: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Port of C++ `_wtoi`: parse a leading base-10 integer (leading
/// whitespace/sign, stops at the first non-digit), returning 0 when there
/// is no number.
fn wtoi(s: &[u16]) -> i32 {
    parse_leading_int(s).unwrap_or(0)
}

/// Parse a leading base-10 integer like `std::stoi` (optional leading
/// whitespace + sign). Returns `None` when no digits are present (the C++
/// `stoi` throws `invalid_argument` in that case).
fn parse_leading_int(s: &[u16]) -> Option<i32> {
    let mut i = 0usize;
    while i < s.len() && is_wspace(s[i]) {
        i += 1;
    }
    let mut sign = 1i32;
    if i < s.len() && (s[i] == b'+' as u16 || s[i] == b'-' as u16) {
        if s[i] == b'-' as u16 {
            sign = -1;
        }
        i += 1;
    }
    let start = i;
    let mut val = 0i32;
    while i < s.len() {
        let c = s[i];
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            break;
        }
        val = val.wrapping_mul(10).wrapping_add((c - b'0' as u16) as i32);
        i += 1;
    }
    if i == start {
        None
    } else {
        Some(sign * val)
    }
}

// --- Attribute map (== C++ `map<wstring,wstring> attribsMap`) --------------

/// The C++ `attribsMap` — a name→value store whose entries are flushed
/// onto the control node at the end of `fillVBuf`. Ordered like
/// `std::map<wstring,wstring>` for stable serialisation.
type Attribs = BTreeMap<Vec<u16>, Vec<u16>>;

/// The `"HTMLAttrib::<name>"` key used for raw HTML attributes.
fn html_key(name: &str) -> Vec<u16> {
    u(&format!("HTMLAttrib::{name}"))
}

/// Look up a `"HTMLAttrib::<name>"` value.
fn attr_html<'a>(attribs: &'a Attribs, name: &str) -> Option<&'a Vec<u16>> {
    attribs.get(&html_key(name))
}

// --- getNameForURL (nvdaHelper/vbufBase/utils.cpp) -------------------------

/// Port of C++ `getNameForURL`. Derives a short human name from a URL.
fn get_name_for_url(url: &[u16]) -> Vec<u16> {
    if url.is_empty() {
        return url.to_vec();
    }
    let colon = b':' as u16;
    let slash = b'/' as u16;
    let hash = b'#' as u16;
    let q = b'?' as u16;
    let dot = b'.' as u16;

    let colon_pos = url.iter().position(|&c| c == colon);
    if let Some(cp) = colon_pos {
        // url.compare(colonPos, 3, L"://") != 0
        let is_scheme_slashes = url.len() >= cp + 3
            && url[cp + 1] == slash
            && url[cp + 2] == slash;
        if !is_scheme_slashes {
            // A protocol that is not path-based (javascript:, mailto:, ...).
            let img_check: Vec<u16> = url[..url.len().min(11)]
                .iter()
                .map(|&c| {
                    if (b'A' as u16..=b'Z' as u16).contains(&c) {
                        c + 32
                    } else {
                        c
                    }
                })
                .collect();
            if img_check.len() >= 11 && img_check[..11] == u("data:image/")[..] {
                return Vec::new();
            }
            // Return the URL with the protocol stripped.
            return url[cp + 1..].to_vec();
        }
    }

    // Path-based protocol or no protocol.
    let query_start = url.iter().rposition(|&c| c == q);
    let mut query_len: Option<usize> = None; // None == "to end of string"
    let anchor_start = url.iter().rposition(|&c| c == hash);
    if let Some(a) = anchor_start {
        // queryLen = anchorStart - queryStart - 1
        if let Some(qs) = query_start {
            query_len = Some(a.wrapping_sub(qs).wrapping_sub(1));
        } else {
            // queryStart is npos; C++ computes anchorStart - npos - 1,
            // but queryLen is only used when queryStart != npos, so its
            // value here is irrelevant.
            query_len = Some(0);
        }
    }

    // pathEnd: sentinel `None` == wstring::npos ("no path").
    let mut strip_exten = true;
    let mut path_end: Option<usize> = if let Some(qs) = query_start {
        Some(qs)
    } else if let Some(a) = anchor_start {
        Some(a)
    } else {
        Some(url.len())
    };
    // pathEnd = (pathEnd >= 0) ? pathEnd - 1 : npos  (pathEnd is unsigned,
    // and url.length() >= 0 always, so this is just pathEnd - 1; when the
    // result would underflow from 0 the C++ wraps to a huge value that is
    // out of range and behaves like npos for the indexing below).
    path_end = path_end.and_then(|pe| pe.checked_sub(1));

    if let Some(pe) = path_end {
        if url[pe] == slash {
            // Path ends with '/': step back one; this component is not a
            // filename, so don't strip the extension.
            path_end = pe.checked_sub(1);
            strip_exten = false;
        }
    }

    let mut path_start = 0usize;
    if let Some(pe) = path_end {
        // Find the start of this path component.
        match url[..=pe].iter().rposition(|&c| c == slash) {
            None => {
                path_start = 0;
            }
            Some(ps) => {
                path_start = ps + 1;
                if strip_exten
                    && colon_pos.is_some()
                    && path_start == colon_pos.unwrap() + 3
                {
                    // Hostname is the last path component; don't strip.
                    strip_exten = false;
                }
            }
        }
        if strip_exten {
            if let Some(es) = url[..=pe].iter().rposition(|&c| c == dot) {
                if es > path_start {
                    path_end = es.checked_sub(1);
                }
            }
        }
    }

    let mut name: Vec<u16> = Vec::new();
    if let Some(pe) = path_end {
        if pe >= path_start && pe < url.len() {
            name.extend_from_slice(&url[path_start..=pe]);
        }
    }
    if let Some(qs) = query_start {
        name.push(b' ' as u16);
        let end = match query_len {
            Some(l) => (qs + 1 + l).min(url.len()),
            None => url.len(),
        };
        if qs < end {
            name.extend_from_slice(&url[qs + 1..end]);
        }
    }
    if let Some(a) = anchor_start {
        name.push(b' ' as u16);
        name.extend_from_slice(&url[a + 1..]);
    }
    // Truncate to 30 chars + ellipsis.
    if name.len() > 30 {
        name.truncate(30);
        name.push(0x2026);
    }
    name
}

// --- getTextFromHTMLDOMNode ------------------------------------------------

/// Port of C++ `getTextFromHTMLDOMNode`. Returns the (possibly
/// whitespace-collapsed) text of a `#text` node, or `None` when the node
/// is not a text node / collapses to nothing.
unsafe fn get_text_from_html_dom_node(
    dom_node: &IHTMLDOMNode,
    allow_preformatted_text: bool,
    is_start_of_block: bool,
) -> Option<Vec<u16>> {
    let text_node: IHTMLDOMTextNode = dom_node.cast().ok()?;
    let data = unsafe { text_node.get_data() }.ok()?;
    let data = data.as_wide();

    if allow_preformatted_text {
        // Preformatted: keep the raw data verbatim.
        return Some(data.to_vec());
    }

    let mut s: Vec<u16> = Vec::new();
    let mut not_all_whitespace = false;
    let mut last_not_whitespace = false;
    let mut stripping_left = is_start_of_block;
    for &c in data {
        if !is_wspace(c) {
            s.push(c);
            last_not_whitespace = true;
            not_all_whitespace = true;
            stripping_left = false;
        } else if last_not_whitespace || !stripping_left {
            s.push(b' ' as u16);
            last_not_whitespace = false;
        }
    }
    if !not_all_whitespace {
        return None;
    }
    Some(s)
}

// --- getCurrentStyleInfoFromHTMLDOMNode ------------------------------------

#[derive(Default)]
struct CurrentStyleInfo {
    dont_render: bool,
    is_block: bool,
    hidden: bool,
    list_style: Vec<u16>,
}

/// Port of C++ `getCurrentStyleInfoFromHTMLDOMNode`. `is_block` defaults
/// to `true` (the C++ caller's initial value), the rest to `false`/empty.
unsafe fn get_current_style_info(dom_node: &IHTMLDOMNode) -> CurrentStyleInfo {
    let mut info = CurrentStyleInfo {
        is_block: true,
        ..Default::default()
    };
    let element2: IHTMLElement2 = match dom_node.cast() {
        Ok(e) => e,
        Err(_) => return info,
    };
    let style: IHTMLCurrentStyle = match unsafe { element2.get_current_style() }
    {
        Ok(s) => s,
        Err(_) => return info,
    };
    // visibility
    if let Ok(v) = unsafe { style.get_visibility() } {
        if !v.is_empty() {
            info.hidden = eq_ic(v.as_wide(), "hidden");
        }
    }
    // display
    if let Ok(d) = unsafe { style.get_display() } {
        if !d.is_empty() {
            let dw = d.as_wide();
            if eq_ic(dw, "none") {
                info.dont_render = true;
                info.is_block = false;
            }
            if eq_ic(dw, "inline") || eq_ic(dw, "inline-block") {
                info.is_block = false;
            }
        }
    }
    // list-style-type
    if let Ok(ls) = unsafe { style.get_list_style_type() } {
        if !ls.is_empty() {
            info.list_style = ls.as_wide().to_vec();
        }
    }
    info
}

// --- getAttributesFromHTMLDOMNode ------------------------------------------

/// C++ `macro_addHTMLAttributeToMap`: read one named attribute and, if
/// present, store it (BSTR honouring `allow_empty`, or VT_I2/VT_I4 as a
/// decimal string) under `"HTMLAttrib::<name>"`.
unsafe fn add_html_attribute_to_map(
    coll: &IHTMLAttributeCollection2,
    attribs: &mut Attribs,
    name: &str,
    allow_empty: bool,
) {
    let attr_node = match unsafe { coll.get_named_item(&BSTR::from(name)) } {
        Ok(a) => a,
        Err(_) => return,
    };
    let v = match unsafe { attr_node.get_node_value() } {
        Ok(v) => v,
        Err(_) => return,
    };
    let raw = v.as_raw();
    let vt = unsafe { raw.Anonymous.Anonymous.vt };
    if vt == VT_BSTR {
        let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
        if !p.is_null() {
            let b = ManuallyDrop::new(unsafe { BSTR::from_raw(p) });
            let w = b.as_wide();
            if allow_empty || !w.is_empty() {
                attribs.insert(html_key(name), w.to_vec());
            }
        }
    } else if vt == VT_I2 || vt == VT_I4 {
        let n = if vt == VT_I2 {
            (unsafe { raw.Anonymous.Anonymous.Anonymous.iVal }) as i32
        } else {
            unsafe { raw.Anonymous.Anonymous.Anonymous.lVal }
        };
        attribs.insert(html_key(name), ui(n));
    }
}

/// Port of C++ `getAttributesFromHTMLDOMNode`.
unsafe fn get_attributes_from_html_dom_node(
    dom_node: &IHTMLDOMNode,
    node_name: &[u16],
    attribs: &mut Attribs,
) {
    let pdisp = match unsafe { dom_node.get_attributes() } {
        Ok(d) => d,
        Err(_) => return,
    };
    let coll: IHTMLAttributeCollection2 = match pdisp.cast() {
        Ok(c) => c,
        Err(_) => return,
    };
    let add = |attribs: &mut Attribs, name: &str, allow_empty: bool| unsafe {
        add_html_attribute_to_map(&coll, attribs, name, allow_empty);
    };

    add(attribs, "id", false);
    if weq(node_name, "TABLE") {
        add(attribs, "summary", false);
    } else if weq(node_name, "A") {
        add(attribs, "href", true);
    } else if weq(node_name, "INPUT") {
        add(attribs, "type", false);
        add(attribs, "value", false);
    } else if weq(node_name, "TD") || weq(node_name, "TH") {
        add(attribs, "headers", false);
        add(attribs, "colspan", false);
        add(attribs, "rowspan", false);
        add(attribs, "scope", false);
    } else if weq(node_name, "OL") {
        add(attribs, "start", false);
    }
    add(attribs, "longdesc", false);
    add(attribs, "alt", true);
    add(attribs, "title", false);
    add(attribs, "src", false);
    // Truncate the value of "src" if it contains base64 data.
    if let Some(src) = attr_html(attribs, "src").cloned() {
        let prefix = u("data:");
        if src.len() >= prefix.len() && src[..prefix.len()] == prefix[..] {
            let needle = u("base64,");
            if let Some(pos) = find_subslice(&src, &needle) {
                let mut newv = src[..pos + needle.len()].to_vec();
                newv.extend_from_slice(&u("<truncated>"));
                attribs.insert(html_key("src"), newv);
            }
        }
    }
    add(attribs, "onclick", false);
    add(attribs, "onmousedown", false);
    add(attribs, "onmouseup", false);
    add(attribs, "required", false);
    add(attribs, "class", true);
    // ARIA properties.
    add(attribs, "role", false);
    add(attribs, "aria-roledescription", false);
    add(attribs, "aria-valuenow", false);
    add(attribs, "aria-sort", false);
    add(attribs, "aria-labelledby", false);
    add(attribs, "aria-describedby", false);
    add(attribs, "aria-expanded", false);
    add(attribs, "aria-selected", false);
    add(attribs, "aria-level", false);
    add(attribs, "aria-required", false);
    add(attribs, "aria-dropeffect", false);
    add(attribs, "aria-grabbed", false);
    add(attribs, "aria-invalid", false);
    add(attribs, "aria-multiline", false);
    add(attribs, "aria-label", false);
    add(attribs, "aria-hidden", false);
    add(attribs, "aria-live", false);
    add(attribs, "aria-relevant", false);
    add(attribs, "aria-busy", false);
    add(attribs, "aria-atomic", false);
    add(attribs, "aria-current", false);
    add(attribs, "aria-placeholder", false);
}

// --- fillTextFormatting ----------------------------------------------------

/// Port of C++ `fillTextFormatting_helper`. Adds `language` (only when
/// non-empty), `formatState` (always), and the `currentStyle`-derived
/// formatting attributes to a text field `node`. `element2` is the
/// element whose `currentStyle` supplies the formatting (the current node
/// for `fillTextFormattingForNode`, the parent element for
/// `fillTextFormattingForTextNode`); `language` / `format_state` are the
/// *parent* control node's values in the C++.
unsafe fn fill_text_formatting_helper(
    element2: &IHTMLElement2,
    node: VbufFieldNode,
    language: &[u16],
    format_state: u32,
) {
    if !language.is_empty() {
        unsafe { node.add_attribute(&u("language"), language) };
    }
    unsafe {
        node.add_attribute(&u("formatState"), &u(&format_state.to_string()))
    };
    let style: IHTMLCurrentStyle = match unsafe { element2.get_current_style() }
    {
        Ok(s) => s,
        Err(_) => return,
    };
    // text-align (BSTR)
    if let Ok(b) = unsafe { style.get_text_align() } {
        if !b.is_empty() {
            unsafe { node.add_attribute(&u("text-align"), b.as_wide()) };
        }
    }
    // font-size (VARIANT → BSTR)
    if let Ok(v) = unsafe { style.get_font_size() } {
        if let Some(w) = variant_bstr(&v) {
            unsafe { node.add_attribute(&u("font-size"), &w) };
        }
    }
    // text-position (verticalAlign VARIANT → BSTR)
    if let Ok(v) = unsafe { style.get_vertical_align() } {
        if let Some(w) = variant_bstr(&v) {
            unsafe { node.add_attribute(&u("text-position"), &w) };
        }
    }
    // font-family (BSTR)
    if let Ok(b) = unsafe { style.get_font_family() } {
        if !b.is_empty() {
            unsafe { node.add_attribute(&u("font-family"), b.as_wide()) };
        }
    }
    // font style
    if let Ok(b) = unsafe { style.get_font_style() } {
        if !b.is_empty() {
            let fs = b.as_wide();
            if !eq_ic(fs, "normal") {
                // name = (fontStyle == "oblique") ? "italic" : fontStyle
                if eq_ic(fs, "oblique") {
                    unsafe { node.add_attribute(&u("italic"), &u("1")) };
                } else {
                    unsafe { node.add_attribute(fs, &u("1")) };
                }
            }
        }
    }
    // font weight (VARIANT VT_I4)
    if let Ok(v) = unsafe { style.get_font_weight() } {
        if let Some(w) = variant_i4(&v) {
            if w >= 700 {
                unsafe { node.add_attribute(&u("bold"), &u("1")) };
            }
        }
    }
    // text decoration (BSTR, may contain multiple space-separated values)
    if let Ok(b) = unsafe { style.get_text_decoration() } {
        if !b.is_empty() {
            let td = b.as_wide();
            if !eq_ic(td, "none") {
                for token in td.split(|&c| c == b' ' as u16) {
                    if token.is_empty() {
                        continue;
                    }
                    // name = (token == "line-through") ? "strikethrough" : token
                    if eq_ic(token, "line-through") {
                        unsafe {
                            node.add_attribute(&u("strikethrough"), &u("1"))
                        };
                    } else {
                        unsafe { node.add_attribute(token, &u("1")) };
                    }
                }
            }
        }
    }
}

/// Port of C++ `fillTextFormattingForNode`: style a text node from the
/// *current* DOM node's element style.
unsafe fn fill_text_formatting_for_node(
    dom_node: &IHTMLDOMNode,
    node: VbufFieldNode,
    language: &[u16],
    format_state: u32,
) {
    let element2: IHTMLElement2 = match dom_node.cast() {
        Ok(e) => e,
        Err(_) => return,
    };
    unsafe {
        fill_text_formatting_helper(&element2, node, language, format_state)
    };
}

/// Port of C++ `fillTextFormattingForTextNode`: text nodes don't support
/// `IHTMLElement2`, so style them from the parent element. In the C++ the
/// parent element comes from `parentNode->pHTMLDOMNode`; here we reach the
/// same element via the text node's DOM parent (they are identical, since
/// a #text node's vbuf parent is its DOM parent).
unsafe fn fill_text_formatting_for_text_node(
    text_dom_node: &IHTMLDOMNode,
    node: VbufFieldNode,
    language: &[u16],
    format_state: u32,
) {
    let parent = match unsafe { text_dom_node.get_parent_node() } {
        Ok(p) => p,
        Err(_) => return,
    };
    let element2: IHTMLElement2 = match parent.cast() {
        Ok(e) => e,
        Err(_) => return,
    };
    unsafe {
        fill_text_formatting_helper(&element2, node, language, format_state)
    };
}

// --- Table state -----------------------------------------------------------

struct TableHeaderInfo {
    unique_id: i32,
    kind: i32,
}

#[derive(Default)]
pub(crate) struct TableInfo {
    table_id: i32,
    cur_row_number: i32,
    cur_column_number: i32,
    definit_data: bool,
    /// Column number → remaining row span.
    column_row_spans: HashMap<i32, i32>,
    /// Column number → accumulated `table-columnheadercells` value.
    column_headers: HashMap<i32, Vec<u16>>,
    /// Row number → accumulated `table-rowheadercells` value.
    row_headers: HashMap<i32, Vec<u16>>,
    /// Header id string → info (for explicit `headers` resolution).
    headers_info: HashMap<Vec<u16>, TableHeaderInfo>,
    /// Cells with an explicit `headers` attribute, resolved when the table
    /// finalises: (cell node, headers attribute value).
    nodes_with_explicit_headers: Vec<(VbufControlFieldNode, Vec<u16>)>,
}

/// Port of C++ `handleColsSpannedByPrevRows`.
fn handle_cols_spanned_by_prev_rows(ti: &mut TableInfo) {
    loop {
        let col = ti.cur_column_number;
        let remaining = match ti.column_row_spans.get_mut(&col) {
            None => return,
            Some(span) => {
                *span -= 1;
                *span
            }
        };
        if remaining == 0 {
            ti.column_row_spans.remove(&col);
        }
        ti.cur_column_number += 1;
    }
}

/// Port of C++ `fillExplicitTableHeadersForCell` (mshtml version: the
/// `headers` attribute is a plain space-separated list of ids).
fn fill_explicit_table_headers_for_cell(
    cell: VbufControlFieldNode,
    doc_handle: i32,
    headers_attr: &[u16],
    ti: &TableInfo,
) {
    let mut col_headers: Vec<u16> = Vec::new();
    let mut row_headers: Vec<u16> = Vec::new();
    let last_pos = headers_attr.len();
    let mut start_pos = 0usize;
    while start_pos < last_pos {
        let end_pos = headers_attr[start_pos..]
            .iter()
            .position(|&c| c == b' ' as u16)
            .map(|off| start_pos + off)
            .unwrap_or(last_pos);
        let id = &headers_attr[start_pos..end_pos];
        start_pos = end_pos + 1;
        if let Some(info) = ti.headers_info.get(id) {
            let entry = u(&format!("{},{};", doc_handle, info.unique_id));
            if info.kind & TABLEHEADER_COLUMN != 0 {
                col_headers.extend_from_slice(&entry);
            }
            if info.kind & TABLEHEADER_ROW != 0 {
                row_headers.extend_from_slice(&entry);
            }
        }
    }
    if !col_headers.is_empty() {
        unsafe {
            cell.as_field_node()
                .add_attribute(&u("table-columnheadercells"), &col_headers)
        };
    }
    if !row_headers.is_empty() {
        unsafe {
            cell.as_field_node()
                .add_attribute(&u("table-rowheadercells"), &row_headers)
        };
    }
}

/// Port of C++ `fillVBuf_helper_collectAndUpdateTableInfo`. Updates the
/// table state (row/column numbering, spans, headers) and writes the
/// corresponding `table-*` entries into `attribs`. Returns the table
/// state to thread to `node`'s descendants (a freshly created one for a
/// `TABLE`, else the inherited one).
fn collect_and_update_table_info(
    table_info: &Option<Rc<RefCell<TableInfo>>>,
    node: VbufControlFieldNode,
    node_name: &[u16],
    doc_handle: i32,
    id: i32,
    attribs: &mut Attribs,
) -> Option<Rc<RefCell<TableInfo>>> {
    // Many in-table elements identify a data table.
    if weq(node_name, "THEAD")
        || weq(node_name, "TFOOT")
        || weq(node_name, "TH")
        || weq(node_name, "CAPTION")
        || weq(node_name, "COLGROUP")
        || weq(node_name, "ROWGROUP")
    {
        if let Some(ti) = table_info {
            ti.borrow_mut().definit_data = true;
        }
    }

    if weq(node_name, "TABLE") {
        let mut ti = TableInfo {
            table_id: id,
            ..Default::default()
        };
        // A summary attribute suggests a data table.
        if attr_html(attribs, "summary").is_some() {
            ti.definit_data = true;
        }
        attribs.insert(u("table-id"), ui(id));
        return Some(Rc::new(RefCell::new(ti)));
    } else if weq(node_name, "TR") {
        if let Some(ti) = table_info {
            let mut ti = ti.borrow_mut();
            ti.cur_row_number += 1;
            ti.cur_column_number = 0;
        }
    }

    if weq(node_name, "TD") || weq(node_name, "TH") {
        if let Some(ti_rc) = table_info {
            handle_cell(ti_rc, node, node_name, doc_handle, id, attribs);
        }
    }

    table_info.clone()
}

/// Cell handling extracted from `collectAndUpdateTableInfo` (mshtml.cpp
/// lines 678-762).
fn handle_cell(
    ti_rc: &Rc<RefCell<TableInfo>>,
    node: VbufControlFieldNode,
    node_name: &[u16],
    doc_handle: i32,
    id: i32,
    attribs: &mut Attribs,
) {
    let mut ti = ti_rc.borrow_mut();
    ti.cur_column_number += 1;
    handle_cols_spanned_by_prev_rows(&mut ti);

    let table_id = ti.table_id;
    let cur_row = ti.cur_row_number;
    let start_col = ti.cur_column_number;
    attribs.insert(u("table-id"), ui(table_id));
    attribs.insert(u("table-rownumber"), ui(cur_row));
    attribs.insert(u("table-columnnumber"), ui(start_col));

    if let Some(headers) = attr_html(attribs, "headers").cloned() {
        // A cell with the headers attribute is definitely a data table.
        ti.definit_data = true;
        // Explicit headers must be recorded later (referenced nodes may
        // not have been rendered yet).
        ti.nodes_with_explicit_headers.push((node, headers));
    } else {
        if let Some(h) = ti.column_headers.get(&start_col).cloned() {
            attribs.insert(u("table-columnheadercells"), h);
        }
        if let Some(h) = ti.row_headers.get(&cur_row).cloned() {
            attribs.insert(u("table-rowheadercells"), h);
        }
    }

    // The last row spanned by this cell (updated below on a row span).
    let mut end_row = cur_row;
    if let Some(colspan) = attr_html(attribs, "colspan").cloned() {
        attribs.insert(u("table-columnsspanned"), colspan.clone());
        ti.cur_column_number += (wtoi(&colspan) - 1).max(0);
    }
    if let Some(rowspan) = attr_html(attribs, "rowspan").cloned() {
        attribs.insert(u("table-rowsspanned"), rowspan.clone());
        let span = wtoi(&rowspan) - 1;
        if span > 0 {
            for col in start_col..=ti.cur_column_number {
                ti.column_row_spans.insert(col, span);
            }
            end_row += span;
        }
    }

    if weq(node_name, "TH") {
        let mut header_type = 0i32;
        if let Some(scope) = attr_html(attribs, "scope") {
            if weq(scope, "col") {
                header_type = TABLEHEADER_COLUMN;
            } else if weq(scope, "row") {
                header_type = TABLEHEADER_ROW;
            } else if weq(scope, "Both") {
                header_type = TABLEHEADER_COLUMN | TABLEHEADER_ROW;
            }
        }
        if header_type == 0 {
            if ti.cur_column_number == 1 {
                header_type = TABLEHEADER_ROW;
            }
            if ti.cur_row_number == 1 {
                header_type |= TABLEHEADER_COLUMN;
            }
        }
        if header_type & TABLEHEADER_COLUMN != 0 {
            let entry = u(&format!("{},{};", doc_handle, id));
            for col in start_col..=ti.cur_column_number {
                ti.column_headers
                    .entry(col)
                    .or_default()
                    .extend_from_slice(&entry);
            }
        }
        if header_type & TABLEHEADER_ROW != 0 {
            let entry = u(&format!("{},{};", doc_handle, id));
            for row in cur_row..=end_row {
                ti.row_headers
                    .entry(row)
                    .or_default()
                    .extend_from_slice(&entry);
            }
        }
        if let Some(id_str) = attr_html(attribs, "id").cloned() {
            ti.headers_info.insert(
                id_str,
                TableHeaderInfo {
                    unique_id: id,
                    kind: header_type,
                },
            );
        }
    }
}

// --- fillVBuf --------------------------------------------------------------

/// Render an MSHTML DOM subtree into `buffer`. Thin public entry point;
/// calls [`fill_vbuf_rec`] with the initial-render defaults.
///
/// # Safety
///
/// `dom_node` must be a live `IHTMLDOMNode`; `buffer` and any node handles
/// must be valid for this render.
pub(crate) unsafe fn fill_vbuf(
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    dom_node: &IHTMLDOMNode,
    ctx: &FillVBufCtx,
) -> Option<VbufFieldNode> {
    unsafe {
        fill_vbuf_rec(
            buffer, parent_node, previous, dom_node, None, None, false, false,
            false, &[], 0, ctx,
        )
    }
}

/// Recursive worker for [`fill_vbuf`]. Mirrors C++
/// `MshtmlVBufBackend_t::fillVBuf` (Phase A). Returns the node to use as
/// the caller's next `previous` sibling: a text node (for the leading
/// #text case) or the created control node, or `None` when nothing was
/// added (script/comment/duplicate/no-id).
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn fill_vbuf_rec(
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    dom_node: &IHTMLDOMNode,
    table_info: Option<Rc<RefCell<TableInfo>>>,
    li_index: Option<Rc<Cell<i32>>>,
    ignore_interactive_unlabelled_graphics: bool,
    mut allow_preformatted_text: bool,
    mut should_skip_text: bool,
    inherited_language: &[u16],
    format_state: u32,
    ctx: &FillVBufCtx,
) -> Option<VbufFieldNode> {
    let doc_handle = ctx.doc_handle;

    // Handle text nodes.
    if !should_skip_text {
        let is_start_of_block = parent_node
            .map(|p| p.as_field_node().is_block())
            .unwrap_or(false)
            && previous.is_none();
        if let Some(s) = unsafe {
            get_text_from_html_dom_node(
                dom_node,
                allow_preformatted_text,
                is_start_of_block,
            )
        } {
            if !s.is_empty() {
                let text_node = unsafe {
                    buffer.add_text_field_node(parent_node, previous, &s)
                }?;
                unsafe {
                    fill_text_formatting_for_text_node(
                        dom_node,
                        text_node,
                        inherited_language,
                        format_state,
                    )
                };
                return Some(text_node);
            }
        }
    }

    // Get node's ID.
    let id = unsafe { get_id_from_html_dom_node(dom_node) };
    if id == 0 {
        return None;
    }
    // Protect against loops.
    if unsafe { buffer.get_control_field_node_with_identifier(doc_handle, id) }
        .is_some()
    {
        return None;
    }

    // Block and visibility style.
    let style = unsafe { get_current_style_info(dom_node) };
    let mut dont_render = style.dont_render;
    let mut hidden = style.hidden;
    let mut is_block = style.is_block;
    let list_style = style.list_style;
    // #4031: nodes hidden due to style should have their direct text
    // nodes skipped.
    if hidden {
        should_skip_text = true;
    }

    // nodeName (uppercased, ASCII).
    let raw_name = match unsafe { dom_node.get_node_name() } {
        Ok(n) => n,
        Err(_) => return None,
    };
    let node_name: Vec<u16> = raw_name
        .as_wide()
        .iter()
        .map(|&c| {
            if (b'a' as u16..=b'z' as u16).contains(&c) {
                c - 32
            } else {
                c
            }
        })
        .collect();

    // Ignore script and comment tags.
    if weq(&node_name, "#COMMENT") || weq(&node_name, "SCRIPT") {
        return None;
    }

    // Only allow linebreaks for 'PRE' tags.
    if weq(&node_name, "PRE") {
        allow_preformatted_text = true;
    }

    let mut attribs: Attribs = BTreeMap::new();
    attribs.insert(u("IHTMLDOMNode::nodeName"), node_name.clone());

    // Collect needed HTML attributes.
    unsafe {
        get_attributes_from_html_dom_node(dom_node, &node_name, &mut attribs)
    };

    // Is this node editable?
    let mut is_editable = false;
    if let Ok(el3) = dom_node.cast::<IHTMLElement3>() {
        if let Ok(vb) = unsafe { el3.get_is_content_editable() } {
            is_editable = vb.as_bool();
        }
        if is_editable {
            attribs.insert(u("IHTMLElement::isContentEditable"), u("1"));
        }
    }

    // aria-hidden.
    if attr_html(&attribs, "aria-hidden").map(|v| weq(v, "true")) == Some(true)
    {
        dont_render = true;
    }

    // input nodes of type hidden must be treated as dontRender.
    if !dont_render
        && weq(&node_name, "INPUT")
        && attr_html(&attribs, "type").map(|v| weq(v, "hidden")) == Some(true)
    {
        dont_render = true;
    }

    // Language: try this DOM node; else, if this is the buffer root, keep
    // walking up the actual DOM (e.g. up to the HTML tag).
    let mut language: Vec<u16> = Vec::new();
    let mut temp: Option<IHTMLDOMNode> = Some(dom_node.clone());
    while let Some(t) = temp.take() {
        if let Ok(el) = t.cast::<IHTMLElement>() {
            if let Ok(v) =
                unsafe { el.get_attribute(&BSTR::from("lang"), 2) }
            {
                if let Some(s) = variant_bstr(&v) {
                    language = s;
                }
            }
        }
        if parent_node.is_none() && language.is_empty() {
            if let Ok(p) = unsafe { t.get_parent_node() } {
                temp = Some(p);
                continue;
            }
        }
        // temp already taken → loop ends.
    }
    if parent_node.is_some() && language.is_empty() {
        language = inherited_language.to_vec();
    }

    // Format state: inherit the parent's, OR in this node's contribution.
    let mut node_format_state = format_state;
    if weq(&node_name, "INS") {
        node_format_state |= FORMATSTATE_INSERTED;
    }
    if weq(&node_name, "DEL") {
        node_format_state |= FORMATSTATE_DELETED;
    }
    if weq(&node_name, "MARK") {
        node_format_state |= FORMATSTATE_MARKED;
    }
    if weq(&node_name, "STRONG") {
        node_format_state |= FORMATSTATE_STRONG;
    }
    if weq(&node_name, "EM") {
        node_format_state |= FORMATSTATE_EMPH;
    }

    // Add the control node to the buffer.
    let node = unsafe {
        buffer.add_control_field_node(
            parent_node,
            previous,
            doc_handle,
            id,
            is_block,
        )
    }?;
    let mut previous_node: Option<VbufFieldNode> = None;

    // All inner parts of a table (rows, cells etc), if changed, must
    // re-render the entire table. Done even for display:none nodes.
    if table_info.is_some()
        && (weq(&node_name, "THEAD")
            || weq(&node_name, "TBODY")
            || weq(&node_name, "TFOOT")
            || weq(&node_name, "TR")
            || weq(&node_name, "TH")
            || weq(&node_name, "TD"))
    {
        unsafe {
            node.set_requires_parent_update(true);
            node.set_allow_reuse_in_ancestor_update(false);
        }
    }

    // Do not render any content for dontRender nodes.
    if dont_render {
        unsafe { node.as_field_node().set_is_hidden(true) };
        return Some(node.as_field_node());
    }

    // Collect available IAccessible information.
    let mut ia_name: Vec<u16> = Vec::new();
    let mut ia_role = 0i32;
    let mut ia_value: Vec<u16> = Vec::new();
    let mut ia_states = 0i32;
    let mut ia_keyboard_shortcut: Vec<u16> = Vec::new();
    let pacc: Option<IAccessible> =
        unsafe { query_service::<IAccessible>(dom_node, &IAccessible::IID) };
    if let Some(pacc) = &pacc {
        let vc = VARIANT::from(0i32);
        ia_name = acc_str(unsafe { pacc.get_accName(&vc) });
        ia_role = unsafe { pacc.get_accRole(&vc) }
            .ok()
            .and_then(|v| variant_i4(&v))
            .unwrap_or(0);
        ia_value = acc_str(unsafe { pacc.get_accValue(&vc) });
        ia_states = unsafe { pacc.get_accState(&vc) }
            .ok()
            .and_then(|v| variant_i4(&v))
            .unwrap_or(0);
        // (accDescription is fetched but unused by the C++, so skipped.)
        ia_keyboard_shortcut =
            acc_str(unsafe { pacc.get_accKeyboardShortcut(&vc) });
    }

    // IE incorrectly places ROLE_SYSTEM_TEXT and no readonly state on
    // unsupported/future tags with an explicit ARIA role.
    if ia_role == ROLE_SYSTEM_TEXT && (ia_states & STATE_SYSTEM_READONLY) == 0 {
        if let Some(role) = attr_html(&attribs, "role") {
            if !weq(role, "textbox") {
                ia_role = ROLE_SYSTEM_STATICTEXT;
            }
        }
    }

    // IE sometimes sets readonly on editable nodes; clear it.
    if is_editable && (ia_states & STATE_SYSTEM_READONLY) != 0 {
        ia_states &= !STATE_SYSTEM_READONLY;
    }

    let mut aria_role: Vec<u16> = Vec::new();
    if let Some(role) = attr_html(&attribs, "role").cloned() {
        aria_role = role;
        if weq(&aria_role, "description") || weq(&aria_role, "search") {
            ia_role = ROLE_SYSTEM_STATICTEXT;
        } else if weq(&aria_role, "list") {
            ia_role = ROLE_SYSTEM_LIST;
            ia_states |= STATE_SYSTEM_READONLY;
        } else if weq(&aria_role, "slider") {
            ia_role = ROLE_SYSTEM_SLIDER;
            ia_states |= STATE_SYSTEM_FOCUSABLE;
            // NOTE: mirrors the C++ lookup of the un-prefixed key
            // "aria-valuenow", which never matches (the map stores it as
            // "HTMLAttrib::aria-valuenow"), so this never fires.
            if let Some(v) = attribs.get(&u("aria-valuenow")) {
                ia_value = v.clone();
            }
        } else if weq(&aria_role, "progressbar") {
            ia_role = ROLE_SYSTEM_PROGRESSBAR;
            if let Some(v) = attribs.get(&u("aria-valuenow")) {
                ia_value = v.clone();
            }
        } else if weq(&aria_role, "application") {
            ia_role = ROLE_SYSTEM_APPLICATION;
        } else if weq(&aria_role, "button") {
            ia_role = ROLE_SYSTEM_PUSHBUTTON;
        } else if weq(&aria_role, "dialog") {
            ia_role = ROLE_SYSTEM_DIALOG;
        } else if !hidden && weq(&aria_role, "presentation") {
            hidden = true;
        }
    }

    // IE doesn't support aria-label; override IAName with it.
    if let Some(label) = attr_html(&attribs, "aria-label").cloned() {
        ia_name = label;
    }

    // IE exposes state_linked for anchors with no href; this is wrong.
    if weq(&node_name, "A") && attr_html(&attribs, "href").is_none() {
        if ia_states & STATE_SYSTEM_LINKED != 0 {
            ia_states &= !STATE_SYSTEM_LINKED;
        }
        if ia_states & STATE_SYSTEM_FOCUSABLE != 0 {
            ia_states &= !STATE_SYSTEM_FOCUSABLE;
        }
    }

    // Whether this is the root node.
    let is_root = id == ctx.root_id;
    // Is this node interactive?
    let mut is_interactive = is_editable
        || (!is_root
            && ia_states & STATE_SYSTEM_FOCUSABLE != 0
            && !weq(&node_name, "BODY")
            && !weq(&node_name, "IFRAME"))
        || (ia_states & STATE_SYSTEM_LINKED != 0)
        || attr_html(&attribs, "onclick").is_some()
        || attr_html(&attribs, "onmouseup").is_some()
        || attr_html(&attribs, "onmousedown").is_some()
        || attr_html(&attribs, "longdesc").is_some();

    // Set up numbering for lists. The counter for an OL frame is threaded
    // to its descendants via `child_li_index`.
    let child_li_index: Option<Rc<Cell<i32>>> = if weq(&node_name, "OL") {
        let mut li = 1i32;
        if let Some(start) = attr_html(&attribs, "start") {
            if !start.is_empty() {
                if let Some(n) = parse_leading_int(start) {
                    li = n;
                }
            }
        }
        Some(Rc::new(Cell::new(li)))
    } else if weq(&node_name, "UL") || weq(&node_name, "DL") {
        None
    } else {
        li_index.clone()
    };

    unsafe { node.as_field_node().set_is_hidden(hidden) };

    // Collect and update table information.
    let child_table = if !hidden {
        collect_and_update_table_info(
            &table_info,
            node,
            &node_name,
            doc_handle,
            id,
            &mut attribs,
        )
    } else {
        table_info.clone()
    };

    // Whether the name is the content of this node.
    let name_is_content = ia_role == ROLE_SYSTEM_LINK
        || ia_role == ROLE_SYSTEM_PUSHBUTTON
        || ia_role == ROLE_SYSTEM_MENUITEM
        || ia_role == ROLE_SYSTEM_GRAPHIC
        || ia_role == ROLE_SYSTEM_PAGETAB
        || weq(&aria_role, "heading")
        || (node_name.len() >= 2
            && node_name[0] == b'H' as u16
            && (b'0' as u16..=b'9' as u16).contains(&node_name[1]))
        || weq(&node_name, "OBJECT")
        || weq(&node_name, "APPLET")
        || (!is_root
            && (ia_role == ROLE_SYSTEM_APPLICATION
                || ia_role == ROLE_SYSTEM_DIALOG));
    // True if the name definitely came from the author.
    let mut name_from_author = false;

    // Opening quote for <Q> elements.
    if weq(&node_name, "Q") {
        if let Some(tn) = unsafe {
            buffer.add_text_field_node(Some(node), previous_node, &u("\u{201c}"))
        } {
            unsafe {
                fill_text_formatting_for_node(
                    dom_node,
                    tn,
                    &language,
                    node_format_state,
                )
            };
            previous_node = Some(tn);
        }
    }

    // Generate content for nodes.
    let mut content_string: Vec<u16> = Vec::new();
    let mut render_children = false;
    if name_is_content
        && (attr_html(&attribs, "aria-label").is_some()
            || attr_html(&attribs, "aria-labelledby").is_some())
    {
        // Explicitly override any content with aria-label(ledby).
        content_string = ia_name.clone();
    } else if weq(&node_name, "HR") {
        content_string = u(" ");
        is_block = true;
        ia_role = ROLE_SYSTEM_SEPARATOR;
    } else if ia_role == ROLE_SYSTEM_SLIDER || ia_role == ROLE_SYSTEM_PROGRESSBAR
    {
        content_string = ia_value.clone();
    } else if weq(&node_name, "OBJECT") || weq(&node_name, "APPLET") {
        is_block = true;
        content_string = u(" ");
    } else if weq(&node_name, "LI") {
        render_children = true;
        if weq(&list_style, "disc")
            || weq(&list_style, "circle")
            || weq(&list_style, "square")
        {
            content_string = u("\u{2022} "); // Bullet
        } else if li_index.is_some()
            && !list_style.is_empty()
            && !weq(&list_style, "none")
        {
            if let Some(li) = &li_index {
                content_string = u(&format!("{}. ", li.get()));
                li.set(li.get() + 1);
            }
        }
    } else if weq(&node_name, "TABLE") {
        render_children = true;
        if let Some(summary) = attr_html(&attribs, "summary") {
            content_string = summary.clone();
        }
    } else if weq(&node_name, "IMG") {
        if let Some(alt) = attr_html(&attribs, "alt").cloned() {
            if alt.is_empty() {
                // alt="", so don't render this at all.
                is_interactive = false;
            } else {
                content_string = alt;
            }
        } else if let Some(title) = attr_html(&attribs, "title").cloned() {
            content_string = title;
        } else if ignore_interactive_unlabelled_graphics {
            is_interactive = false;
        } else if is_interactive && !ia_value.is_empty() {
            // Unlabelled graphic; derive a name from the URL.
            content_string = get_name_for_url(&ia_value);
        }
    } else if weq(&node_name, "INPUT") {
        let input_type = attr_html(&attribs, "type").cloned();
        if input_type.as_deref().map(|t| weq(t, "file")) == Some(true) {
            content_string = ia_value.clone();
            content_string.extend_from_slice(&u("..."));
            ia_role = ROLE_SYSTEM_PUSHBUTTON;
        } else if ia_role == ROLE_SYSTEM_TEXT {
            // accValue can fail on protected fields; fall back to the
            // value attribute.
            let value_attr = attr_html(&attribs, "value").cloned();
            if ia_value.is_empty() {
                if let Some(v) = value_attr {
                    content_string = v;
                }
            } else {
                content_string = ia_value.clone();
            }
            if ia_states & STATE_SYSTEM_PROTECTED != 0 {
                content_string.iter_mut().for_each(|c| *c = b'*' as u16);
            }
            name_from_author = true;
        } else if ia_role == ROLE_SYSTEM_PUSHBUTTON {
            content_string = ia_name.clone();
        } else if ia_role == ROLE_SYSTEM_RADIOBUTTON
            || ia_role == ROLE_SYSTEM_CHECKBUTTON
        {
            name_from_author = true;
        }
        if content_string.is_empty() {
            content_string = u(" ");
        }
    } else if weq(&node_name, "SELECT") {
        content_string = if !ia_value.is_empty() {
            ia_value.clone()
        } else {
            u(" ")
        };
        name_from_author = true;
    } else if weq(&node_name, "TEXTAREA") {
        is_block = true;
        content_string = if !ia_value.is_empty() {
            ia_value.clone()
        } else {
            u(" ")
        };
        name_from_author = true;
    } else if weq(&node_name, "BR") {
        content_string = u("\n");
    } else if (!is_root
        && (ia_role == ROLE_SYSTEM_APPLICATION || ia_role == ROLE_SYSTEM_DIALOG))
        || ia_role == ROLE_SYSTEM_OUTLINE
        || weq(&node_name, "MATH")
    {
        content_string = u(" ");
    } else {
        render_children = true;
    }

    // If the name isn't the content, add it as a field attribute when it
    // came from the author (not content).
    if !name_is_content
        && !ia_name.is_empty()
        && (name_from_author
            || attr_html(&attribs, "aria-label").is_some()
            || attr_html(&attribs, "aria-labelledby").is_some()
            || attr_html(&attribs, "title").is_some()
            || attr_html(&attribs, "alt").is_some())
    {
        attribs.insert(u("name"), ia_name.clone());
        attribs.insert(u("alwaysReportName"), u("true"));
    }

    // Add a text node for any special content retrieved.
    if !hidden && !content_string.is_empty() {
        if let Some(tn) = unsafe {
            buffer.add_text_field_node(Some(node), previous_node, &content_string)
        } {
            unsafe {
                fill_text_formatting_for_node(
                    dom_node,
                    tn,
                    &language,
                    node_format_state,
                )
            };
            previous_node = Some(tn);
        }
    }

    // Record IAccessible information as attributes.
    if !ia_keyboard_shortcut.is_empty() {
        attribs.insert(u("keyboardShortcut"), ia_keyboard_shortcut);
    }
    attribs.insert(u("IAccessible::role"), ui(ia_role));
    for i in 0..32 {
        let state = 1i32 << i;
        if state & ia_states != 0 {
            attribs.insert(u(&format!("IAccessible::state_{}", state)), u("1"));
        }
    }

    // Render children if allowed.
    if render_children {
        let mut ignore_graphics = ignore_interactive_unlabelled_graphics;
        if is_interactive && !ignore_graphics {
            // Don't render interactive unlabelled graphic descendants if
            // this node has a name (author names are preferred).
            ignore_graphics = !ia_name.is_empty();
        }

        if weq(&node_name, "FRAME") || weq(&node_name, "IFRAME") {
            // Children of frames come via IAccessible.
            if let Some(pacc) = &pacc {
                if let Some(child_dom) =
                    unsafe { get_frame_body(pacc) }
                {
                    previous_node = unsafe {
                        fill_vbuf_rec(
                            buffer,
                            Some(node),
                            previous_node,
                            &child_dom,
                            child_table.clone(),
                            child_li_index.clone(),
                            ignore_graphics,
                            allow_preformatted_text,
                            should_skip_text,
                            &language,
                            node_format_state,
                            ctx,
                        )
                    };
                }
            }
        } else if let Ok(pdisp) = unsafe { dom_node.get_child_nodes() } {
            if let Ok(coll) = pdisp.cast::<IHTMLDOMChildrenCollection>() {
                let length = unsafe { coll.get_length() }.unwrap_or(0);
                for i in 0..length {
                    let child_disp = match unsafe { coll.item(i) } {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    let child_dom: IHTMLDOMNode = match child_disp.cast() {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    if let Some(tn) = unsafe {
                        fill_vbuf_rec(
                            buffer,
                            Some(node),
                            previous_node,
                            &child_dom,
                            child_table.clone(),
                            child_li_index.clone(),
                            ignore_graphics,
                            allow_preformatted_text,
                            should_skip_text,
                            &language,
                            node_format_state,
                            ctx,
                        )
                    } {
                        previous_node = Some(tn);
                    }
                }
            }
        }

        // A node whose rendered children produce no useful content should
        // render its title or URL.
        if !hidden && !unsafe { node.as_field_node().has_useful_content() } {
            let mut fallback: Vec<u16> = Vec::new();
            if !ia_name.is_empty() {
                fallback = ia_name.clone();
            }
            if fallback.is_empty() {
                if let Some(title) = attr_html(&attribs, "title") {
                    fallback = title.clone();
                }
            }
            if fallback.is_empty() {
                if let Some(href) = attr_html(&attribs, "href") {
                    if !href.is_empty() {
                        fallback = get_name_for_url(href);
                    }
                }
            }
            if !fallback.is_empty() {
                if let Some(tn) = unsafe {
                    buffer.add_text_field_node(Some(node), None, &fallback)
                } {
                    unsafe {
                        fill_text_formatting_for_node(
                            dom_node,
                            tn,
                            &language,
                            node_format_state,
                        )
                    };
                    previous_node = Some(tn);
                }
            }
            // If any descendant is invalidated, this may change whether
            // this node has useful content.
            unsafe { node.set_always_rerender_descendants(true) };
        }
    }

    // Update attributes with table info (finalise the table).
    if !hidden && weq(&node_name, "TABLE") {
        if let Some(ti_rc) = &child_table {
            let ti = ti_rc.borrow();
            if !ti.definit_data {
                attribs.insert(u("table-layout"), u("1"));
            }
            for (cell, headers) in &ti.nodes_with_explicit_headers {
                fill_explicit_table_headers_for_cell(
                    *cell, doc_handle, headers, &ti,
                );
            }
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("table-rowcount"), &ui(ti.cur_row_number));
                node.as_field_node().add_attribute(
                    &u("table-columncount"),
                    &ui(ti.cur_column_number),
                );
            }
        }
    }

    if !hidden {
        // Table cells are always at least a space; if just a space, they
        // are not block.
        if (weq(&node_name, "TD") || weq(&node_name, "TH"))
            && unsafe { node.as_field_node().get_length() } == 0
        {
            is_block = false;
            unsafe {
                buffer.add_text_field_node(Some(node), previous_node, &u(" "))
            };
        }
        // An interactive node with no content gets a space.
        if is_interactive
            && unsafe { node.as_field_node().get_length() } == 0
        {
            unsafe {
                buffer.add_text_field_node(Some(node), previous_node, &u(" "))
            };
        }
    }

    // Update block setting.
    unsafe { node.as_field_node().set_is_block(is_block) };

    // Flush all collected attributes onto the node.
    for (name, value) in &attribs {
        unsafe { node.as_field_node().add_attribute(name, value) };
    }
    // The C++ `generateAttributesForMarkupOpeningTag` override always
    // appends `language="..."`; emit it as a normal attribute here.
    unsafe { node.as_field_node().add_attribute(&u("language"), &language) };

    // Closing quote for <Q> elements.
    if weq(&node_name, "Q") {
        if let Some(tn) = unsafe {
            buffer.add_text_field_node(Some(node), previous_node, &u("\u{201d}"))
        } {
            unsafe {
                fill_text_formatting_for_node(
                    dom_node,
                    tn,
                    &language,
                    node_format_state,
                )
            };
        }
    }

    Some(node.as_field_node())
}

/// Port of C++ `getIDFromHTMLDOMNode`: the node's `IHTMLUniqueName`
/// unique number, or 0.
unsafe fn get_id_from_html_dom_node(dom_node: &IHTMLDOMNode) -> i32 {
    let unique: IHTMLUniqueName = match dom_node.cast() {
        Ok(u) => u,
        Err(_) => return 0,
    };
    unsafe { unique.get_unique_number() }.unwrap_or(0)
}

/// Port of C++ `getHTMLSubdocumentBodyFromIAccessibleFrame`: the frame's
/// child document body, reached via `IAccessible::get_accChild(1)` then
/// `queryService(IID_IHTMLElement)` returning an `IHTMLDOMNode`.
pub(crate) unsafe fn get_frame_body(pacc: &IAccessible) -> Option<IHTMLDOMNode> {
    let vc = VARIANT::from(1i32);
    let pdisp = unsafe { pacc.get_accChild(&vc) }.ok()?;
    unsafe { query_service::<IHTMLDOMNode>(&pdisp, &IHTMLElement::IID) }
}
