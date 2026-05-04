//! Port of `nvdaHelper/remote/ia2LiveRegions.cpp`.
//!
//! For now this module exposes only the pure attribute predicates over
//! the IA2 attribute map. The COM-orchestration helpers
//! (`find_aria_atomic`, `is_in_background_tab`, the event handler, and
//! the `extern "C"` shim) are added in follow-up commits.

use std::collections::BTreeMap;

use crate::attribs::parse_attribs;
use crate::interfaces::IAccessible2;
use windows::core::Interface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePoliteness {
    Polite,
    Assertive,
    Rude,
}

impl LivePoliteness {
    /// The `container-live` attribute value that yielded this politeness.
    /// The same string is forwarded to `nvdaControllerInternal_reportLiveRegion`.
    pub fn as_str(&self) -> &'static str {
        match self {
            LivePoliteness::Polite => "polite",
            LivePoliteness::Assertive => "assertive",
            LivePoliteness::Rude => "rude",
        }
    }
}

/// Read the `container-live` IA2 attribute and map it to a
/// [`LivePoliteness`] if the value is one the live-region hook
/// recognises. Mirrors the predicate at `ia2LiveRegions.cpp:147-148`.
pub fn parse_live_politeness(map: &BTreeMap<String, String>) -> Option<LivePoliteness> {
    match map.get("container-live")?.as_str() {
        "polite" => Some(LivePoliteness::Polite),
        "assertive" => Some(LivePoliteness::Assertive),
        "rude" => Some(LivePoliteness::Rude),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relevance {
    pub additions: bool,
    pub text: bool,
}

/// Read the `container-relevant` IA2 attribute. Mirrors the parsing at
/// `ia2LiveRegions.cpp:176-185`. Absent / `all` -> additions+text;
/// otherwise look for the words `additions` and `text`.
pub fn parse_container_relevant(map: &BTreeMap<String, String>) -> Relevance {
    match map.get("container-relevant") {
        None => Relevance { additions: true, text: true },
        Some(v) if v == "all" => Relevance { additions: true, text: true },
        Some(v) => Relevance {
            additions: v.contains("additions"),
            text: v.contains("text"),
        },
    }
}

/// Mirrors `ia2LiveRegions.cpp:171-172`.
pub fn is_container_busy(map: &BTreeMap<String, String>) -> bool {
    map.get("container-busy").map(|v| v == "true").unwrap_or(false)
}

/// Mirrors `ia2LiveRegions.cpp:31-32`.
pub fn is_atomic(map: &BTreeMap<String, String>) -> bool {
    map.get("atomic").map(|v| v == "true").unwrap_or(false)
}

/// Mirrors `ia2LiveRegions.cpp:38-39`.
pub fn is_container_atomic(map: &BTreeMap<String, String>) -> bool {
    map.get("container-atomic").map(|v| v == "true").unwrap_or(false)
}

/// If `pacc2` declares `atomic="true"`, returns it (cloned, AddRef'd).
/// Otherwise, if it declares `container-atomic="true"`, walks up
/// `accParent` and returns the nearest atomic ancestor (recursively).
/// Returns `None` if no atomic ancestor exists.
///
/// Mirrors `findAriaAtomic` in `ia2LiveRegions.cpp:30-56`.
///
/// `attribs_map` is the IA2 attributes for `pacc2` -- the caller already
/// has these for the entry node, so we take them as a parameter rather
/// than fetching twice.
///
/// # Safety
///
/// `pacc2` must be a live, well-formed `IAccessible2` for the duration
/// of the call. The recursive walk dereferences each parent pointer the
/// COM server returns.
pub unsafe fn find_aria_atomic(
    pacc2: &IAccessible2,
    attribs_map: &BTreeMap<String, String>,
) -> Option<IAccessible2> {
    if is_atomic(attribs_map) {
        return Some(pacc2.clone());
    }
    if !is_container_atomic(attribs_map) {
        return None;
    }
    // Walk up to the parent. accParent returns IDispatch; QI to
    // IAccessible2.
    let parent_disp = unsafe { pacc2.accParent() }.ok()?;
    let parent_acc2: IAccessible2 = parent_disp.cast().ok()?;
    let parent_bstr = unsafe { parent_acc2.get_attributes() }.ok()?;
    // BSTR -> String works for both null and zero-length BSTRs (both
    // produce ""), and parse_attribs("") returns an empty map. The
    // recursion bails on that empty map at the next is_container_atomic
    // check.
    let parent_map = parse_attribs(&parent_bstr.to_string());
    unsafe { find_aria_atomic(&parent_acc2, &parent_map) }
}

use windows::core::VARIANT;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{IDispatch, IServiceProvider};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

/// IA2 navigation relations. See `include/ia2/api/IA2Relations.idl`. These
/// constants are not exported by windows-rs, so we declare them locally.
pub(crate) const NAVRELATION_EMBEDS: i32 = 0x1009;
pub(crate) const NAVRELATION_CONTAINING_TAB_PANE: i32 = 0x1012;

/// Pull the IA2 `uniqueID` out of a VARIANT that should hold an IDispatch
/// pointing to an `IAccessible`. Mirrors `getIa2UniqueIdFromDispatchVariant`
/// in `ia2LiveRegions.cpp:58-74`. Returns `0` for any failure path
/// (matches the C++ contract; the caller compares against another id and
/// `0` falls through to "unknown" treatment).
pub fn ia2_unique_id_from_dispatch_variant(variant: &VARIANT) -> i32 {
    let Ok(disp) = IDispatch::try_from(variant) else { return 0 };
    let Ok(serv) = disp.cast::<IServiceProvider>() else { return 0 };
    // SAFETY: QueryService is FFI; we hold a live IServiceProvider via
    // `serv` for the duration of the call.
    let Ok(acc2) = (unsafe { serv.QueryService::<IAccessible2>(&IAccessible::IID) })
    else {
        return 0;
    };
    // SAFETY: acc2 is a live IAccessible2 we just received from QueryService.
    unsafe { acc2.get_uniqueID() }.unwrap_or(0)
}

/// Returns `true` if `pacc2` lives in a Firefox background tab. Mirrors
/// `isInBackgroundTab` in `ia2LiveRegions.cpp:76-107`.
///
/// In Firefox, all tabs share the same HWND. The "containing tab pane"
/// for the event target is compared against the "embedded" tab pane on
/// the window root: if they have different IA2 unique IDs, the event
/// target is in a background tab.
///
/// # Safety
///
/// `pacc2` must be a live `IAccessible2`; `hwnd` must be a valid window
/// handle for the duration of the call.
pub unsafe fn is_in_background_tab(pacc2: &IAccessible2, hwnd: HWND) -> bool {
    let pacc: &IAccessible = pacc2;
    let start = VARIANT::from(0i32); // CHILDID_SELF
    let acc_doc = match unsafe { pacc.accNavigate(NAVRELATION_CONTAINING_TAB_PANE, &start) } {
        Ok(v) => v,
        Err(_) => return false,
    };
    let acc_doc_id = ia2_unique_id_from_dispatch_variant(&acc_doc);
    if acc_doc_id == 0 {
        return false;
    }
    // Get the root accessible for the window.
    let mut root_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
    if unsafe {
        AccessibleObjectFromWindow(
            hwnd,
            OBJID_CLIENT.0 as u32,
            &IAccessible::IID,
            &mut root_ptr,
        )
    }
    .is_err()
    {
        return false;
    }
    if root_ptr.is_null() {
        return false;
    }
    // Take ownership of the AddRef'd IAccessible the out-param contract
    // gave us. `from_raw` consumes the raw pointer's reference; `root`'s
    // Drop (i.e. `Release`) balances it. Mirrors the C++ CComPtr.
    let root: IAccessible = unsafe { IAccessible::from_raw(root_ptr) };
    let fg_doc = match unsafe { root.accNavigate(NAVRELATION_EMBEDS, &start) } {
        Ok(v) => v,
        Err(_) => return false,
    };
    let fg_doc_id = ia2_unique_id_from_dispatch_variant(&fg_doc);
    if fg_doc_id == 0 {
        return false;
    }
    acc_doc_id != fg_doc_id
}

use crate::text::get_text_from_iaccessible_collect;
use crate::types::IA2_STATE_EDITABLE;
use windows::Win32::UI::Controls::STATE_SYSTEM_OFFSCREEN;

/// WinEvent identifiers the live-region hook cares about. Pre-filtered by
/// the C++ side so Rust doesn't need to import the platform constants.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveRegionEvent {
    NameChange = 0,
    DescriptionChange = 1,
    Show = 2,
    TextInserted = 3,
    TextUpdated = 4,
}

impl LiveRegionEvent {
    fn from_raw(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::NameChange),
            1 => Some(Self::DescriptionChange),
            2 => Some(Self::Show),
            3 => Some(Self::TextInserted),
            4 => Some(Self::TextUpdated),
            _ => None,
        }
    }
}

/// C-callable callback invoked once at the end of
/// [`nvda_ia2_handle_live_region_event`] when the event passes all filters.
/// Mirrors the AttribCallback / AppendCharsCallback pattern from earlier
/// FFI shims.
///
/// # Safety
///
/// The callback must not unwind. Both pointers are valid for their
/// respective `_len` `u16` elements; the callback must copy the data
/// before returning.
pub type ReportLiveRegionCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    text_ptr: *const u16,
    text_len: usize,
    politeness_ptr: *const u16,
    politeness_len: usize,
);

/// C-callable replacement for the IA2 portion of `winEventProcHook`.
///
/// `pacc2` is borrowed (no `Release`). `event_kind` is one of the
/// [`LiveRegionEvent`] discriminants; any other value yields `false`
/// without invoking the callback. `acc_state` is the IA2 state bitmask
/// the C++ side fetched (passes `0` if the source VARIANT was not
/// `VT_I4`). `report_cb` is invoked at most once, only when the event
/// passes every filter.
///
/// # Safety
///
/// * `pacc2` must be a valid `IAccessible2*` for the duration of the call.
/// * `hwnd` must be a valid `HWND` (or null; the background-tab check
///   bails on a null window).
/// * `report_cb` must be a valid function pointer; `ctx` is opaque user
///   data. `report_cb` must not unwind.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_handle_live_region_event(
    pacc2: *mut core::ffi::c_void,
    hwnd: *mut core::ffi::c_void,
    event_kind: u32,
    acc_state: i32,
    ctx: *mut core::ffi::c_void,
    report_cb: ReportLiveRegionCallback,
) -> bool {
    if pacc2.is_null() {
        return false;
    }
    let acc2: &IAccessible2 = match IAccessible2::from_raw_borrowed(&pacc2) {
        Some(a) => a,
        None => return false,
    };
    let event = match LiveRegionEvent::from_raw(event_kind) {
        Some(e) => e,
        None => return false,
    };
    let hwnd = HWND(hwnd);
    handle_live_region_event(acc2, hwnd, event, acc_state, ctx, report_cb)
}

unsafe fn handle_live_region_event(
    pacc2: &IAccessible2,
    hwnd: HWND,
    event: LiveRegionEvent,
    acc_state: i32,
    ctx: *mut core::ffi::c_void,
    report_cb: ReportLiveRegionCallback,
) -> bool {
    // Fetch IA2 attributes; bail if unavailable.
    let attribs_bstr = unsafe { pacc2.get_attributes() }.ok();
    let attribs_map: BTreeMap<String, String> = match attribs_bstr {
        Some(b) => parse_attribs(&b.to_string()),
        None => return false,
    };

    // container-live filter.
    let politeness = match parse_live_politeness(&attribs_map) {
        Some(p) => p,
        None => return false,
    };

    // Background-tab filter (only when the offscreen state bit is set).
    // STATE_SYSTEM_OFFSCREEN is wrapped by windows-rs as a tuple struct
    // (the wrapper type name is unrelated to its actual semantics --
    // it's the standard Win32 STATE_SYSTEM_* bitmask value).
    if (acc_state & STATE_SYSTEM_OFFSCREEN.0 as i32) != 0
        && unsafe { is_in_background_tab(pacc2, hwnd) }
    {
        return false;
    }

    // IA2_STATE_EDITABLE filter -- editable text should never announce
    // as a live region (typed characters would be echoed twice, etc.).
    let ia2_states = unsafe { pacc2.get_states() }.unwrap_or(0);
    if (ia2_states & IA2_STATE_EDITABLE) != 0 {
        return false;
    }

    // container-busy filter.
    if is_container_busy(&attribs_map) {
        return false;
    }

    // container-relevant parse.
    let relevance = parse_container_relevant(&attribs_map);
    if !relevance.additions && !relevance.text {
        return false;
    }

    // Show events only flow through if additions are allowed.
    if event == LiveRegionEvent::Show && !relevance.additions {
        return false;
    }

    // Show edge case: ignore the event if there's a parent we can text
    // through OR the parent has its own valid container-live (we are
    // not the root of the region).
    if event == LiveRegionEvent::Show && unsafe { should_ignore_show_event(pacc2) } {
        return false;
    }

    // Name/description changes only flow through if text is allowed.
    if !relevance.text
        && (event == LiveRegionEvent::NameChange
            || event == LiveRegionEvent::DescriptionChange)
    {
        return false;
    }

    // Resolve the text.
    let mut text_buf: Vec<u16> = Vec::new();
    let pacc2_atomic = unsafe { find_aria_atomic(pacc2, &attribs_map) };
    let got_text = if let Some(atomic) = pacc2_atomic.as_ref() {
        get_text_from_iaccessible_collect(&mut text_buf, atomic, false, true, true)
    } else {
        match event {
            LiveRegionEvent::NameChange => {
                let varchild = VARIANT::from(0i32);
                let pacc: &IAccessible = pacc2;
                if let Ok(name) = unsafe { pacc.get_accName(&varchild) } {
                    text_buf.extend_from_slice(name.as_wide());
                    true
                } else {
                    false
                }
            }
            LiveRegionEvent::DescriptionChange => {
                let varchild = VARIANT::from(0i32);
                let pacc: &IAccessible = pacc2;
                if let Ok(desc) = unsafe { pacc.get_accDescription(&varchild) } {
                    text_buf.extend_from_slice(desc.as_wide());
                    true
                } else {
                    false
                }
            }
            LiveRegionEvent::Show => get_text_from_iaccessible_collect(
                &mut text_buf, pacc2, false, true, true,
            ),
            LiveRegionEvent::TextInserted | LiveRegionEvent::TextUpdated => {
                get_text_from_iaccessible_collect(
                    &mut text_buf,
                    pacc2,
                    true,
                    relevance.additions,
                    relevance.text,
                )
            }
        }
    };

    if !got_text || text_buf.is_empty() {
        return false;
    }

    let politeness_str = politeness.as_str();
    let politeness_utf16: Vec<u16> = politeness_str.encode_utf16().collect();
    unsafe {
        report_cb(
            ctx,
            text_buf.as_ptr(),
            text_buf.len(),
            politeness_utf16.as_ptr(),
            politeness_utf16.len(),
        );
    }
    true
}

unsafe fn should_ignore_show_event(pacc2: &IAccessible2) -> bool {
    let pacc: &IAccessible = pacc2;
    let parent_disp = match unsafe { pacc.accParent() } {
        Ok(d) => d,
        Err(_) => return false,
    };
    // If the parent has IAccessibleText, the upcoming text events handle
    // this better -- ignore the show event.
    if parent_disp.cast::<crate::interfaces::IAccessibleText>().is_ok() {
        return true;
    }
    // Otherwise, default to "we are the root, do not ignore" unless the
    // parent has a valid container-live, in which case we are not the
    // root.
    let parent_acc2: IAccessible2 = match parent_disp.cast() {
        Ok(a) => a,
        Err(_) => return true, // No IA2 on parent -> assume we are root
    };
    let parent_bstr = match unsafe { parent_acc2.get_attributes() } {
        Ok(b) => b,
        Err(_) => return true,
    };
    let parent_map = parse_attribs(&parent_bstr.to_string());
    parse_live_politeness(&parent_map).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn live_politeness_absent_is_none() {
        assert_eq!(parse_live_politeness(&map(&[])), None);
    }

    #[test]
    fn live_politeness_off_is_none() {
        assert_eq!(parse_live_politeness(&map(&[("container-live", "off")])), None);
    }

    #[test]
    fn live_politeness_polite() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "polite")])),
            Some(LivePoliteness::Polite),
        );
    }

    #[test]
    fn live_politeness_assertive() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "assertive")])),
            Some(LivePoliteness::Assertive),
        );
    }

    #[test]
    fn live_politeness_rude() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "rude")])),
            Some(LivePoliteness::Rude),
        );
    }

    #[test]
    fn live_politeness_unknown_is_none() {
        assert_eq!(
            parse_live_politeness(&map(&[("container-live", "loud")])),
            None,
        );
    }

    #[test]
    fn relevant_absent_defaults_to_all() {
        assert_eq!(
            parse_container_relevant(&map(&[])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_all_explicit() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "all")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_additions_only() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "additions")])),
            Relevance { additions: true, text: false },
        );
    }

    #[test]
    fn relevant_text_only() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "text")])),
            Relevance { additions: false, text: true },
        );
    }

    #[test]
    fn relevant_additions_and_text() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "additions text")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_text_and_additions() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "text additions")])),
            Relevance { additions: true, text: true },
        );
    }

    #[test]
    fn relevant_unrecognized_is_neither() {
        assert_eq!(
            parse_container_relevant(&map(&[("container-relevant", "removals")])),
            Relevance { additions: false, text: false },
        );
    }

    #[test]
    fn busy_true() {
        assert!(is_container_busy(&map(&[("container-busy", "true")])));
    }

    #[test]
    fn busy_false_value() {
        assert!(!is_container_busy(&map(&[("container-busy", "false")])));
    }

    #[test]
    fn busy_absent() {
        assert!(!is_container_busy(&map(&[])));
    }

    #[test]
    fn atomic_true() {
        assert!(is_atomic(&map(&[("atomic", "true")])));
    }

    #[test]
    fn atomic_absent() {
        assert!(!is_atomic(&map(&[])));
    }

    #[test]
    fn container_atomic_true() {
        assert!(is_container_atomic(&map(&[("container-atomic", "true")])));
    }

    #[test]
    fn container_atomic_absent() {
        assert!(!is_container_atomic(&map(&[])));
    }
}
