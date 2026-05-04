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
