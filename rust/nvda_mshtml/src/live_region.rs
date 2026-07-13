//! ARIA live-region auto-announcement for the MSHTML vbuf backend.
//!
//! Port of `preProcessLiveRegion` / `postProcessLiveRegion` /
//! `reportLiveText` / `reportLiveAddition` from
//! `nvdaHelper/vbufBackends/mshtml/node.cpp` (lines 380-505), plus the
//! `fillVBuf` call-site plumbing from `mshtml.cpp` (~919, ~1350-1357,
//! ~1404-1408). The threading through the render itself lives in
//! [`crate::fill_vbuf`]; this module holds the state type and the
//! compute/report functions.
//!
//! # Deliberate simplifications vs the C++
//!
//! * **String-level text diff.** `postProcessLiveRegion`'s C++ diff walks
//!   the old/new nodes' text *children* pairwise (skipping empties /
//!   containers), finds the first and last differing text child, and
//!   concatenates the new text between them. This port instead diffs the
//!   two nodes' *flattened* text (`get_text_in_range` over the whole
//!   node): strip the common UTF-16 prefix and suffix, and take the middle
//!   of the new text as the changed run. This avoids widening the shared
//!   storage API (no child-level walk over `Buffer`), and yields the same
//!   "what's new" text for the common append/replace cases.
//! * **`isNodeInLiveRegion` filter ignored.** `reportLiveAddition` passes a
//!   `getTextInRange` filter that prunes descendant control nodes which are
//!   themselves separate live regions. We report the node's full flattened
//!   text unfiltered; nested independent live regions inside a reported
//!   addition are rare and the filter only trims their text from the
//!   announcement.
//! * **Re-render-root inherited state via DOM walk-up.** The C++ seeds the
//!   re-render root's inherited live state from `oldNode->getParent()` — a
//!   node in the *old* tree whose `LiveState` the Rust storage does not
//!   retain. [`walk_up_parent_live_state`] instead walks the root DOM
//!   node's ancestor elements (reading `aria-live` / `aria-relevant` /
//!   `aria-busy` / `aria-atomic` via `getAttribute`), mirroring the
//!   existing `language` DOM walk-up in `fill_vbuf`. Because an
//!   `aria-live` / `aria-atomic` ancestor found this way is outside the
//!   re-rendered subtree (not a vbuf node), `live_root` is left `None`
//!   (region membership is instead signalled by a non-empty `politeness`)
//!   and an atomic ancestor is represented by the re-render root node
//!   itself as a reportable stand-in.

use core::cell::RefCell;
use std::rc::Rc;

use windows::core::{Interface, BSTR};

use nvda_vbuf::VbufControlFieldNode;

use crate::fill_vbuf::{
    attr_html, find_subslice, is_wspace, u, variant_bstr, weq, Attribs,
    FillVBufCtx,
};
use crate::interfaces::{IHTMLDOMNode, IHTMLElement};

extern "system" {
    /// `error_status_t __stdcall nvdaControllerInternal_reportLiveRegion(
    ///     const wchar_t* text, const wchar_t* politeness)`.
    ///
    /// Declared `extern "system"` so the ABI matches the C++ `__stdcall`
    /// on x86 (identical to `extern "C"` on x64 / ARM64). The symbol
    /// resolves from `nvdaControllerInternal_C.obj` when the aggregate
    /// staticlib is linked into `nvdaHelperRemote.dll`.
    fn nvdaControllerInternal_reportLiveRegion(
        text: *const u16,
        level: *const u16,
    ) -> u32;
}

/// Shared collector for `aria-atomic` ancestors flagged during a render,
/// each paired with the politeness its full-text announcement should use.
/// Drained once by the backend adapter after `fill_vbuf` returns.
pub(crate) type AtomicNodes =
    Rc<RefCell<Vec<(VbufControlFieldNode, Vec<u16>)>>>;

/// The live-region state threaded down the `fill_vbuf` recursion, mirroring
/// the `ariaLive*` fields the C++ stores on each control node.
#[derive(Clone, Default)]
pub struct LiveState {
    /// The nearest ancestor (or self) that declared an *active*
    /// (`polite` / `assertive`) `aria-live`, when that node is part of the
    /// currently rendered subtree; `None` otherwise. Only its *identity*
    /// matters (the `ariaLiveNode != this` test); region membership is
    /// carried by [`Self::politeness`] so the DOM-walk-up stand-in (where
    /// the live root is an un-rendered ancestor) can leave this `None`.
    pub live_root: Option<VbufControlFieldNode>,
    /// The live level (`"polite"` / `"assertive"`), or empty when not in an
    /// active live region. Non-empty is the faithful proxy for the C++
    /// `ariaLiveNode != nullptr`: the C++ keeps `ariaLivePoliteness` even
    /// for `aria-live="off"`, but only ever reads it when `ariaLiveNode`
    /// is set, so this port normalises a disabled `aria-live` to an empty
    /// politeness.
    pub politeness: Vec<u16>,
    /// `aria-relevant` includes `text` (default `true`).
    pub text_relevant: bool,
    /// `aria-relevant` includes `additions` (default `true`).
    pub additions_relevant: bool,
    /// `aria-busy == "true"` (default `false`).
    pub busy: bool,
    /// The nearest `aria-atomic="true"` ancestor (or self) as a reportable
    /// vbuf node, or `None`.
    pub atomic_node: Option<VbufControlFieldNode>,
    /// The politeness [`reportLiveAddition`](Self::atomic_node) should use
    /// for [`Self::atomic_node`] — that atomic node's *own* politeness, not
    /// the changed node's. Captured here because the Rust storage does not
    /// retain a node's `LiveState`, so the deferred atomic drain (which
    /// runs after `fill_vbuf`) has no other way to recover it. Matches the
    /// C++ `reportLiveAddition` reading the atomic node's
    /// `ariaLivePoliteness`.
    pub atomic_politeness: Vec<u16>,
}

/// `true` when two control-field handles denote the same node.
fn same_node(a: VbufControlFieldNode, b: VbufControlFieldNode) -> bool {
    core::ptr::eq(a.0.buffer as *const _, b.0.buffer as *const _)
        && a.0.key == b.0.key
}

/// Port of `MshtmlVBufStorage_controlFieldNode_t::preProcessLiveRegion`.
/// Computes `node`'s live-region state from its own `aria-*` attributes,
/// inheriting from `parent` when an attribute is absent.
pub(crate) fn pre_process_live_region(
    node: VbufControlFieldNode,
    attribs: &Attribs,
    parent: &LiveState,
) -> LiveState {
    let mut s = LiveState::default();

    // aria-live
    match attr_html(attribs, "aria-live") {
        Some(v) if !v.is_empty() => {
            let enabled = weq(v, "polite") || weq(v, "assertive");
            s.live_root = if enabled { Some(node) } else { None };
            // See `LiveState::politeness`: disabled -> empty so that
            // "politeness non-empty" == the C++ `ariaLiveNode != null`.
            s.politeness = if enabled { v.clone() } else { Vec::new() };
        }
        _ => {
            s.live_root = parent.live_root;
            s.politeness = parent.politeness.clone();
        }
    }

    // aria-relevant
    match attr_html(attribs, "aria-relevant") {
        Some(v) if !v.is_empty() => {
            if weq(v, "all") {
                s.text_relevant = true;
                s.additions_relevant = true;
            } else {
                s.text_relevant = find_subslice(v, &u("text")).is_some();
                s.additions_relevant =
                    find_subslice(v, &u("additions")).is_some();
            }
        }
        _ => {
            s.text_relevant = parent.text_relevant;
            s.additions_relevant = parent.additions_relevant;
        }
    }

    // aria-busy
    match attr_html(attribs, "aria-busy") {
        Some(v) if !v.is_empty() => s.busy = weq(v, "true"),
        _ => s.busy = parent.busy,
    }

    // aria-atomic
    match attr_html(attribs, "aria-atomic") {
        Some(v) if !v.is_empty() => {
            if weq(v, "true") {
                s.atomic_node = Some(node);
                s.atomic_politeness = s.politeness.clone();
            } else {
                s.atomic_node = None;
                s.atomic_politeness = Vec::new();
            }
        }
        _ => {
            s.atomic_node = parent.atomic_node;
            s.atomic_politeness = parent.atomic_politeness.clone();
        }
    }

    s
}

/// Read an `aria-*` attribute off a DOM element (case-insensitive, via
/// `getAttribute(name, 2)`), returning its string value or `None`.
unsafe fn read_aria(el: &IHTMLElement, name: &str) -> Option<Vec<u16>> {
    let v = unsafe { el.get_attribute(&BSTR::from(name), 2) }.ok()?;
    variant_bstr(&v)
}

/// Compute the inherited [`LiveState`] for the re-render root by walking
/// its DOM ancestor elements, standing in for the C++
/// `preProcessLiveRegion(oldNode->getParent(), ...)`. Each `aria-*`
/// attribute is resolved at its nearest-declaring ancestor (mirroring how
/// the threaded C++ inheritance would have settled it). See the module
/// docs for why `live_root` stays `None` and an atomic ancestor is
/// represented by `root_node`.
pub(crate) unsafe fn walk_up_parent_live_state(
    root_dom: &IHTMLDOMNode,
    root_node: VbufControlFieldNode,
) -> LiveState {
    // Defaults match the C++ `parent ? ... : <default>` fall-throughs.
    let mut s = LiveState {
        text_relevant: true,
        additions_relevant: true,
        ..Default::default()
    };
    let mut live_found = false;
    let mut relevant_found = false;
    let mut busy_found = false;
    let mut atomic_found = false;

    let mut cur = unsafe { root_dom.get_parent_node() }.ok();
    while let Some(node) = cur {
        if let Ok(el) = node.cast::<IHTMLElement>() {
            if !live_found {
                if let Some(v) = unsafe { read_aria(&el, "aria-live") } {
                    if !v.is_empty() {
                        live_found = true;
                        let enabled = weq(&v, "polite") || weq(&v, "assertive");
                        // live_root stays None (the ancestor is outside the
                        // rendered subtree); politeness carries membership.
                        s.politeness = if enabled { v } else { Vec::new() };
                    }
                }
            }
            if !relevant_found {
                if let Some(v) = unsafe { read_aria(&el, "aria-relevant") } {
                    if !v.is_empty() {
                        relevant_found = true;
                        if weq(&v, "all") {
                            s.text_relevant = true;
                            s.additions_relevant = true;
                        } else {
                            s.text_relevant =
                                find_subslice(&v, &u("text")).is_some();
                            s.additions_relevant =
                                find_subslice(&v, &u("additions")).is_some();
                        }
                    }
                }
            }
            if !busy_found {
                if let Some(v) = unsafe { read_aria(&el, "aria-busy") } {
                    if !v.is_empty() {
                        busy_found = true;
                        s.busy = weq(&v, "true");
                    }
                }
            }
            if !atomic_found {
                if let Some(v) = unsafe { read_aria(&el, "aria-atomic") } {
                    if !v.is_empty() {
                        atomic_found = true;
                        if weq(&v, "true") {
                            // The atomic ancestor is outside the subtree;
                            // report the re-render root as its stand-in.
                            s.atomic_node = Some(root_node);
                            s.atomic_politeness = s.politeness.clone();
                        }
                    }
                }
            }
        }
        if live_found && relevant_found && busy_found && atomic_found {
            break;
        }
        cur = unsafe { node.get_parent_node() }.ok();
    }

    // If an atomic ancestor resolved before a nearer aria-live did, refresh
    // its stand-in politeness from the finally-resolved region politeness.
    if s.atomic_node.is_some() {
        s.atomic_politeness = s.politeness.clone();
    }
    s
}

/// The flattened text of `node` (`getTextInRange(0, length)`), read from
/// the node's own buffer.
pub(crate) unsafe fn full_text(node: VbufControlFieldNode) -> Vec<u16> {
    let len = unsafe { node.as_field_node().get_length() };
    let mut out: Vec<u16> = Vec::new();
    if len > 0 {
        unsafe {
            (*node.0.buffer).get_text_in_range(node.0.key, 0, len, &mut out)
        };
    }
    out
}

/// String-level replacement for the C++ child-by-child diff: strip the
/// common UTF-16 prefix and suffix of the old and new flattened texts and
/// return the middle of the *new* text (the changed / added run).
unsafe fn changed_text(
    old: VbufControlFieldNode,
    new: VbufControlFieldNode,
) -> Vec<u16> {
    let old_text = unsafe { full_text(old) };
    let new_text = unsafe { full_text(new) };
    let ol = old_text.len();
    let nl = new_text.len();
    let mut pre = 0usize;
    while pre < ol && pre < nl && old_text[pre] == new_text[pre] {
        pre += 1;
    }
    let mut suf = 0usize;
    while suf < ol - pre
        && suf < nl - pre
        && old_text[ol - 1 - suf] == new_text[nl - 1 - suf]
    {
        suf += 1;
    }
    new_text[pre..nl - suf].to_vec()
}

/// Port of `reportLiveText`: announce `text` at `politeness`, unless `text`
/// is empty or all whitespace.
pub(crate) fn report_live_text(text: &[u16], politeness: &[u16]) {
    if text.iter().copied().all(is_wspace) {
        return;
    }
    let mut t = text.to_vec();
    t.push(0);
    let mut p = politeness.to_vec();
    p.push(0);
    unsafe {
        nvdaControllerInternal_reportLiveRegion(t.as_ptr(), p.as_ptr());
    }
}

/// Record `atomic` (with the politeness `reportLiveAddition` should use)
/// for the deferred post-render drain, de-duplicated by node identity.
fn push_atomic(ctx: &FillVBufCtx, atomic: VbufControlFieldNode, politeness: &[u16]) {
    let mut v = ctx.atomic_nodes.borrow_mut();
    if !v.iter().any(|(n, _)| same_node(*n, atomic)) {
        v.push((atomic, politeness.to_vec()));
    }
}

/// Port of `MshtmlVBufStorage_controlFieldNode_t::postProcessLiveRegion`.
/// Decides what (if anything) to announce for `node` on a re-render, given
/// its old counterpart `old_node` (`None` when the node is newly added).
pub(crate) unsafe fn post_process_live_region(
    node: VbufControlFieldNode,
    old_node: Option<VbufControlFieldNode>,
    state: &LiveState,
    ctx: &FillVBufCtx,
) {
    // if(!ariaLiveNode || ariaLiveIsBusy) return;
    if state.politeness.is_empty() || state.busy {
        return;
    }

    // reportNode = !oldNode && additionsRelevant && ariaLiveNode != this
    let report_node = old_node.is_none()
        && state.additions_relevant
        && state.live_root.map(|r| !same_node(r, node)).unwrap_or(true);

    // Text diff for an existing node (if not reporting the whole node).
    let mut changed = Vec::new();
    if !report_node {
        if let Some(old) = old_node {
            if state.text_relevant {
                changed = unsafe { changed_text(old, node) };
            }
        }
    }

    if !report_node && changed.is_empty() {
        return;
    }

    if let Some(atomic) = state.atomic_node {
        // An atomic ancestor swallows the change: report the whole atomic
        // node (deferred), nothing more.
        push_atomic(ctx, atomic, &state.atomic_politeness);
    } else if report_node {
        // reportLiveAddition: this node's full text, at its politeness.
        let text = unsafe { full_text(node) };
        report_live_text(&text, &state.politeness);
    } else {
        report_live_text(&changed, &state.politeness);
    }
}
