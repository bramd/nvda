//! Rust port of `GeckoVBufBackend_t::fillVBuf` from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:408`.
//!
//! Carved into blocks per the design doc at
//! `docs/plans/2026-05-06-rust-fill-vbuf-design.md`. The Rust-internal
//! entry point is [`fill_vbuf`]; as of the Phase 6e flip (Stage D) it is
//! driven from the update orchestration in
//! [`crate::gecko_backend_state::nvda_ia2_gecko_backend_update`], which
//! renders straight into the backend's embedded Rust `storage::Buffer`
//! (initial render) or a temp `Buffer` (partial render, reuse querying
//! the live buffer via `FillVBufCtx.main`). The former C-callable
//! `nvda_ia2_fill_vbuf` extern (which the old C++ `render()` delegated
//! to) was removed at the flip — see the note where it used to live.
//!
//! Recursion stays Rust-side: [`fill_vbuf`] (the Rust internal
//! function) calls itself directly. The C++ side does not see the
//! recursive calls.
//!
//! All seven blocks are implemented; [`fill_vbuf`] is a complete port of
//! the C++ original.
//!
//! Block carve-up (lines refer to gecko_ia2.cpp before the flip):
//!
//! | Block | Title                                         | Lines        |
//! | :---: | --------------------------------------------- | ------------ |
//! |   1   | entry, identity, IA2 attribs, role            | 408-514      |
//! |   2   | states, name, locale, derived booleans        | 514-627      |
//! |   3   | render flags, actions, label info             | 628-755      |
//! |   4   | table state plumbing                          | 757-908      |
//! |   5   | recursive content (text seg / children walk)  | 911-1130     |
//! |   6   | non-recursive content fallbacks               | 1131-1202    |
//! |   7   | name attr / desc-is-content / aria-details    | 1206-1245    |
#![allow(dead_code)]

use std::collections::BTreeMap;

use crate::acc_description::get_acc_description_native;
use crate::aria_details::fill_vbuf_aria_details_native;
use crate::aria_error::fill_vbuf_aria_error_native;
use crate::attribs::parse_attribs;
use crate::bstr::is_bstr_null;
use crate::child_count::get_child_count_native;
use crate::fetch::fetch_ia2_attributes_native;
use crate::hyperlink_getter::HyperlinkGetter;
use crate::interfaces::{
    IAccessible2, IAccessibleAction, IAccessibleHypertext,
    IAccessibleHypertext2, IAccessibleTable2, IAccessibleTableCell,
    IAccessibleText,
};
use crate::label_info::{get_label_info_native, LabelInfo};
use crate::name_for_url::get_name_for_url;
use crate::role_long_string::get_role_long_role_string_native;
use crate::selected_item::get_selected_item;
use crate::table_cell::{fill_table_cell_info_native, get_table_id_from_cell};
use crate::textbox_in_combobox::get_text_box_in_combo_box;
use crate::utf16::utf16;
use nvda_vbuf::{VbufBackend, VbufBuffer, VbufControlFieldNode, VbufFieldNode};
use windows::core::{Interface, VARIANT};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::UI::Accessibility::{AccessibleChildren, IAccessible};

/// MSAA `ROLE_SYSTEM_OUTLINE` — listed in `oleacc.h` as `0x23` (35).
/// Hard-coded to avoid pulling another windows-rs feature in for a
/// single `i32`.
const ROLE_SYSTEM_OUTLINE: i32 = 0x23;
/// `ROLE_SYSTEM_TABLE` — `oleacc.h` value `0x18` (24).
const ROLE_SYSTEM_TABLE: i32 = 0x18;
/// `ROLE_SYSTEM_EQUATION` — `oleacc.h` value `0x35` (53).
const ROLE_SYSTEM_EQUATION: i32 = 0x35;
/// `ROLE_SYSTEM_GRAPHIC` — `oleacc.h` value `0x28` (40).
const ROLE_SYSTEM_GRAPHIC: i32 = 0x28;
/// `ROLE_SYSTEM_TEXT` — `oleacc.h` value `0x2a` (42).
const ROLE_SYSTEM_TEXT: i32 = 0x2a;
/// `ROLE_SYSTEM_DOCUMENT` — `oleacc.h` value `0x0f` (15).
const ROLE_SYSTEM_DOCUMENT: i32 = 0x0f;
/// `ROLE_SYSTEM_DIALOG` — `oleacc.h` value `0x12` (18).
const ROLE_SYSTEM_DIALOG: i32 = 0x12;
/// `ROLE_SYSTEM_APPLICATION` — `oleacc.h` value `0x0e` (14).
const ROLE_SYSTEM_APPLICATION: i32 = 0x0e;
/// `ROLE_SYSTEM_CELL` — `oleacc.h` value `0x1d` (29).
const ROLE_SYSTEM_CELL: i32 = 0x1d;
/// `ROLE_SYSTEM_ROW` — `oleacc.h` value `0x1c` (28).
const ROLE_SYSTEM_ROW: i32 = 0x1c;
/// `ROLE_SYSTEM_SEPARATOR` — `oleacc.h` value `0x15` (21).
const ROLE_SYSTEM_SEPARATOR: i32 = 0x15;
/// `ROLE_SYSTEM_LIST` — `oleacc.h` value `0x21` (33).
const ROLE_SYSTEM_LIST: i32 = 0x21;
/// `ROLE_SYSTEM_LINK` — `oleacc.h` value `0x1e` (30).
const ROLE_SYSTEM_LINK: i32 = 0x1e;
/// `ROLE_SYSTEM_PUSHBUTTON` — `oleacc.h` value `0x2b` (43).
const ROLE_SYSTEM_PUSHBUTTON: i32 = 0x2b;
/// `ROLE_SYSTEM_MENUITEM` — `oleacc.h` value `0x0c` (12).
const ROLE_SYSTEM_MENUITEM: i32 = 0x0c;
/// `ROLE_SYSTEM_PAGETAB` — `oleacc.h` value `0x25` (37).
const ROLE_SYSTEM_PAGETAB: i32 = 0x25;
/// `ROLE_SYSTEM_BUTTONMENU` — `oleacc.h` value `0x39` (57).
const ROLE_SYSTEM_BUTTONMENU: i32 = 0x39;
/// `ROLE_SYSTEM_CHECKBUTTON` — `oleacc.h` value `0x2c` (44).
const ROLE_SYSTEM_CHECKBUTTON: i32 = 0x2c;
/// `ROLE_SYSTEM_RADIOBUTTON` — `oleacc.h` value `0x2d` (45).
const ROLE_SYSTEM_RADIOBUTTON: i32 = 0x2d;
/// `ROLE_SYSTEM_COMBOBOX` — `oleacc.h` value `0x2e` (46).
const ROLE_SYSTEM_COMBOBOX: i32 = 0x2e;

/// IA2-specific role values, derived sequentially from
/// `IA2_ROLE_CANVAS = 0x401` per `AccessibleRole.idl` (verified against
/// `build/<arch>/ia2.h`).
const IA2_ROLE_UNKNOWN: i32 = 0x0;
const IA2_ROLE_EMBEDDED_OBJECT: i32 = 0x40a;
const IA2_ROLE_HEADING: i32 = 0x414;
const IA2_ROLE_INTERNAL_FRAME: i32 = 0x418;
const IA2_ROLE_SECTION: i32 = 0x424;
const IA2_ROLE_TEXT_FRAME: i32 = 0x429;
const IA2_ROLE_TOGGLE_BUTTON: i32 = 0x42a;

/// MSAA states from `oleacc.h`.
const STATE_SYSTEM_LINKED: i32 = 0x40_0000;
const STATE_SYSTEM_FOCUSABLE: i32 = 0x10_0000;
const STATE_SYSTEM_UNAVAILABLE: i32 = 0x1;
const STATE_SYSTEM_READONLY: i32 = 0x40;
const STATE_SYSTEM_INDETERMINATE: i32 = 0x20;

/// `ROLE_SYSTEM_PROGRESSBAR` — `oleacc.h` value `0x30` (48).
const ROLE_SYSTEM_PROGRESSBAR: i32 = 0x30;
/// `ROLE_SYSTEM_ROWHEADER` — `oleacc.h` value `0x1a` (26).
const ROLE_SYSTEM_ROWHEADER: i32 = 0x1a;
/// `ROLE_SYSTEM_COLUMNHEADER` — `oleacc.h` value `0x19` (25).
const ROLE_SYSTEM_COLUMNHEADER: i32 = 0x19;

/// Single-space text used as the "empty content" placeholder. Mirrors
/// `EMPTY_TEXT_NODE` in gecko_ia2.cpp.
const EMPTY_TEXT_NODE: &[u16] = &utf16(b" ");

/// IA2 state bits from `AccessibleStates.idl`.
const IA2_STATE_EDITABLE: i32 = 0x8;
const IA2_STATE_MULTI_LINE: i32 = 0x200;

// Constant UTF-16 attribute names, prefixes, and values, encoded at
// compile time via `crate::utf16::utf16`. fillVBuf writes these on
// nearly every node across thousands of nodes per render, so encoding
// them per call would allocate tens of thousands of transient buffers.
const ATTR_ROLE: &[u16] = &utf16(b"IAccessible::role");
const ATTR_KEYBOARD_SHORTCUT: &[u16] = &utf16(b"keyboardShortcut");
const ATTR_DESCRIPTION: &[u16] = &utf16(b"description");
const ATTR_ALWAYS_REPORT_NAME: &[u16] = &utf16(b"alwaysReportName");
const ATTR_NAME: &[u16] = &utf16(b"name");
const ATTR_LANGUAGE: &[u16] = &utf16(b"language");
const ATTR_TEXT_ALIGN: &[u16] = &utf16(b"text-align");
const ATTR_LABELLED_BY_CONTENT: &[u16] = &utf16(b"labelledByContent");
const ATTR_DESCRIPTION_IS_CONTENT: &[u16] = &utf16(b"descriptionIsContent");
const ATTR_IA_VALUE: &[u16] = &utf16(b"IAccessible::value");
const ATTR_TABLE_LAYOUT: &[u16] = &utf16(b"table-layout");
const ATTR_TABLE_ID: &[u16] = &utf16(b"table-id");
const ATTR_TABLE_ROWCOUNT: &[u16] = &utf16(b"table-rowcount");
const ATTR_TABLE_COLUMNCOUNT: &[u16] = &utf16(b"table-columncount");
const ATTR_TABLE_ROWNUMBER_PRES: &[u16] =
    &utf16(b"table-rownumber-presentational");
const ATTR_TABLE_COLUMNNUMBER_PRES: &[u16] =
    &utf16(b"table-columnnumber-presentational");
const ATTR_TABLE_ROWCOUNT_PRES: &[u16] =
    &utf16(b"table-rowcount-presentational");
const ATTR_TABLE_COLUMNCOUNT_PRES: &[u16] =
    &utf16(b"table-columncount-presentational");
const ATTR_IA2_TEXT_START_OFFSET: &[u16] = &utf16(b"ia2TextStartOffset");
const ATTR_IA2_TEXT_WINDOW_HANDLE: &[u16] = &utf16(b"ia2TextWindowHandle");
const ATTR_IA2_TEXT_UNIQUE_ID: &[u16] = &utf16(b"ia2TextUniqueID");
const PREFIX_IA_STATE: &[u16] = &utf16(b"IAccessible::state_");
const PREFIX_IA2_STATE: &[u16] = &utf16(b"IAccessible2::state_");
const PREFIX_IA2_ATTRIBUTE: &[u16] = &utf16(b"IAccessible2::attribute_");
const PREFIX_IA_ACTION: &[u16] = &utf16(b"IAccessibleAction_");
const VAL_TRUE: &[u16] = &utf16(b"true");
const VAL_ONE: &[u16] = &utf16(b"1");

/// Outcome of [`block1`] indicating how the caller should proceed.
pub enum Block1Outcome {
    /// `pacc` lacked a window handle, lacked a unique ID, or the
    /// (docHandle, ID) pair was already in the buffer (loop guard).
    /// `fillVBuf` should return null.
    Bail,
    /// Cross-buffer reuse fired: a reference node was added to
    /// `buffer` pointing at an existing node owned by `backend`.
    /// `fillVBuf` should return this node directly without
    /// continuing.
    Reused(VbufFieldNode),
    /// A new control field node was created and the IA2 attribute
    /// prelude (IAccessible2::attribute_*, IAccessible::role) is
    /// already populated. `fillVBuf` continues with the rest of the
    /// processing using the returned context.
    Continue(Block1Continue),
}

/// State produced by [`block1`] when processing should continue. All
/// fields mirror the C++ original's local variables at line 514, just
/// after the role attribute has been written.
pub struct Block1Continue {
    /// The new control field node added to the buffer. The C++ code
    /// re-uses the variable name `parentNode` for this past line 466.
    pub parent_node: VbufControlFieldNode,
    /// IA2 unique ID of `pacc`.
    pub id: i32,
    /// Document handle (truncated `HWND`).
    pub doc_handle: i32,
    /// Post-normalization IA2 role (treegrid override applied,
    /// equation-with-img-tag override applied).
    pub role: i32,
    /// The `roleAttr` C++ wstring -- either the BSTR from the MSAA
    /// fallback or a decimal stringification of `role`. Downstream
    /// code (notably `fillVBufAriaDetails`) consumes this.
    pub role_attr: Vec<u16>,
    /// Parsed IA2 attributes. Used downstream for many lookups
    /// (`display`, `formatting`, `tag`, `src`, etc.).
    pub attribs: BTreeMap<String, String>,
}

/// Block 1 of `fillVBuf`: get docHandle/ID, dedup, cross-buffer reuse,
/// add the new control field node, populate IA2 attributes, normalize
/// role, set the role attribute.
///
/// Mirrors lines 408-514 of `gecko_ia2.cpp` exactly — see the source
/// for the rationale behind each step.
///
/// # Safety
///
/// `pacc` must point at a live `IAccessible2`; `buffer`, `ctx.backend`,
/// `ctx.main`, `parent`, `previous` (if `Some`) must be live for the
/// duration.
pub unsafe fn block1(
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    ctx: &FillVBufCtx,
    parent: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
) -> Block1Outcome {
    // Block 1 / step 1: docHandle from get_windowHandle. The C++
    // original truncates HWND to int via HandleToUlong; we follow.
    let hwnd = match unsafe { pacc.get_windowHandle() } {
        Ok(h) => h,
        Err(_) => return Block1Outcome::Bail,
    };
    let doc_handle: i32 = (hwnd.0 as usize) as i32;
    if doc_handle == 0 {
        return Block1Outcome::Bail;
    }

    // Block 1 / step 2: IA2 unique ID.
    let id = match unsafe { pacc.get_uniqueID() } {
        Ok(i) => i,
        Err(_) => return Block1Outcome::Bail,
    };

    // Block 1 / step 3: loop guard -- bail if already known.
    if unsafe {
        buffer.get_control_field_node_with_identifier(doc_handle, id)
    }
    .is_some()
    {
        return Block1Outcome::Bail;
    }

    // Block 1 / step 4: cross-buffer reuse. The C++ original gates
    // this on `buffer != this && parentNode`, where `this` is the
    // GeckoVBufBackend_t (which IS-A buffer). `reuse_existing_node`
    // folds in the "temp buffer, not the live/main one" guard (see the
    // helper); here we only need the parent guard.
    if let Some(parent_node) = parent {
        if let Some(existing) = unsafe {
            reuse_existing_node(
                ctx, buffer, parent_node, previous, doc_handle, id,
            )
        } {
            // Mirrors the C++ early `return
            // buffer->addReferenceNodeToBuffer(...)`: whatever this
            // produces (including NULL) becomes fillVBuf's return.
            let reference = unsafe {
                buffer.add_reference_node(Some(parent_node), previous, existing)
            };
            return match reference {
                Some(r) => Block1Outcome::Reused(r),
                None => Block1Outcome::Bail,
            };
        }
    }

    // Block 1 / step 5: add the new control field node. The C++ code
    // reassigns `parentNode` to the new node and resets `previousNode`
    // to NULL. Here block1 only produces the new control field node
    // (returned as `cont.parent_node`); the `previousNode = NULL` reset
    // is done by the caller `fill_vbuf`, which shadows `previous` to
    // `None` before running blocks 4-7. Do NOT thread the incoming
    // sibling `previous` into this node's children -- its parent is the
    // grandparent, and the storage rejects such an anchor.
    let new_parent_node = match unsafe {
        buffer.add_control_field_node(parent, previous, doc_handle, id, true)
    } {
        Some(n) => n,
        None => {
            // The C++ original asserts non-NULL here (`nhAssert(parentNode)`).
            // On a debug build this would terminate the process; on a
            // release build it falls through to a NULL-deref shortly
            // after. We bail gracefully.
            return Block1Outcome::Bail;
        }
    };

    // Block 1 / step 6: IA2 attributes. A NULL BSTR (no attributes)
    // is collapsed to an empty map here — this path doesn't need the
    // NULL-vs-empty distinction the FFI shim preserves.
    let attribs = fetch_ia2_attributes_native(pacc).unwrap_or_default();
    apply_ia2_attribs_to_node(new_parent_node, &attribs);

    // Block 1 / step 7: role + role normalization.
    let (mut role, role_string) =
        get_role_long_role_string_native(pacc, 0);
    if role == ROLE_SYSTEM_OUTLINE
        && has_xml_role_attrib_containing_value(&attribs, "treegrid")
    {
        role = ROLE_SYSTEM_TABLE;
    }
    if role == ROLE_SYSTEM_EQUATION
        && attribs.get("tag").map(|v| v.as_str()) == Some("img")
    {
        role = ROLE_SYSTEM_GRAPHIC;
    }

    // Block 1 / step 8: roleAttr -- BSTR from fallback or decimal role.
    let role_attr: Vec<u16> = match role_string {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            use core::fmt::Write;
            let _ = write!(buf, "{role}");
            buf.encode_utf16().collect()
        }
    };

    // Set IAccessible::role attribute.
    unsafe {
        new_parent_node
            .as_field_node()
            .add_attribute(ATTR_ROLE, &role_attr);
    }

    Block1Outcome::Continue(Block1Continue {
        parent_node: new_parent_node,
        id,
        doc_handle,
        role,
        role_attr,
        attribs,
    })
}

/// Cross-buffer reuse lookup for [`block1`]. Returns `Some(existing)`
/// pointing at a reuse-eligible node in the backend's live/main storage,
/// or `None` when reuse doesn't apply.
///
/// The reuse query runs against the Rust main `Buffer` (`ctx.main`)
/// directly; `buffer` is the temp render buffer. Returns `None` for an
/// initial render (where `buffer` *is* the main storage), mirroring the
/// C++ `buffer != this` guard — an initial render has nothing to reuse
/// against and `buffer` must be a distinct allocation from `ctx.main`
/// otherwise.
///
/// # Safety
///
/// All borrowed handles must be live; `parent_node` / `previous` belong
/// to `buffer` (the in-flight render target).
unsafe fn reuse_existing_node(
    ctx: &FillVBufCtx,
    buffer: VbufBuffer,
    parent_node: VbufControlFieldNode,
    previous: Option<VbufFieldNode>,
    doc_handle: i32,
    id: i32,
) -> Option<VbufControlFieldNode> {
    // `ctx.main` is the live Rust buffer. An initial render targets it
    // directly (buffer == main); reuse only applies when `buffer` is a
    // distinct temp buffer.
    if buffer.0 == ctx.main.0 {
        return None;
    }
    unsafe {
        ctx.main.reuse_existing_node_in_render(
            buffer,
            Some(parent_node),
            previous,
            doc_handle,
            id,
        )
    }
}

// ----- block 2 ------------------------------------------------------

/// Mutable per-element state populated by [`block2`] and consumed by
/// later blocks. Mirrors the locals declared in the C++ original at
/// gecko_ia2.cpp:516-627.
pub struct Block2State {
    /// Result of `IAccessible::accName`. `None` when the call failed
    /// or returned a NULL BSTR; an empty BSTR is preserved as
    /// `Some(vec![])`.
    pub name: Option<Vec<u16>>,
    /// Result of `getAccDescription`. Already written to the node as
    /// the `description` attribute when present; downstream blocks
    /// consult it for `descriptionIsContent` detection.
    pub description: Option<Vec<u16>>,
    /// Assembled locale string -- "lang", "lang-country", or empty.
    /// Downstream blocks attach it to text nodes via the `language`
    /// attribute.
    pub locale: Vec<u16>,
    /// MSAA state bitmask (`accState`).
    pub states: i32,
    /// IA2 state bitmask (`get_states`), with `IA2_STATE_EDITABLE`
    /// removed for ARIA grids per gecko_ia2.cpp:541-544.
    pub ia2_states: i32,
    pub is_editable: bool,
    pub in_link: bool,
    pub is_root: bool,
    pub is_embedded_app: bool,
    pub is_never_interactive: bool,
    /// Tentative interactive flag. Refined later when actions are
    /// inspected (block 3) -- those mutate this in place.
    pub is_interactive: bool,
}

/// Block 2 of `fillVBuf`: states, keyboard shortcut, isBlock /
/// isHidden, name / description / locale, derived booleans.
/// Mirrors lines 516-627 of `gecko_ia2.cpp`.
///
/// # Safety
///
/// `pacc` must be live for the duration; `parent_node` must be a
/// live control field node owned by the buffer.
pub unsafe fn block2(
    pacc: &IAccessible2,
    parent_node: VbufControlFieldNode,
    role: i32,
    attribs: &BTreeMap<String, String>,
    id: i32,
    ctx: &FillVBufCtx,
) -> Block2State {
    let pacc_msaa: &IAccessible = pacc;
    let varchild = VARIANT::from(0i32);

    // MSAA states: get_accState returns a VT_I4 VARIANT with the
    // state bitmask. On failure, fall back to 0.
    let states: i32 = match unsafe { pacc_msaa.get_accState(&varchild) } {
        Ok(v) => {
            let raw: VARIANT = v;
            let raw = raw.as_raw();
            let vt = unsafe { raw.Anonymous.Anonymous.vt };
            if vt == VT_I4_RAW {
                unsafe { raw.Anonymous.Anonymous.Anonymous.lVal }
            } else {
                0
            }
        }
        Err(_) => 0,
    };
    write_state_attributes(parent_node, PREFIX_IA_STATE, states);

    // IA2 states. Strip IA2_STATE_EDITABLE on tables (Gecko exposes
    // it for ARIA grids, which is not in the ARIA spec).
    let mut ia2_states: i32 = unsafe { pacc.get_states() }.unwrap_or(0);
    if (ia2_states & IA2_STATE_EDITABLE) != 0 && role == ROLE_SYSTEM_TABLE {
        ia2_states -= IA2_STATE_EDITABLE;
    }
    write_state_attributes(parent_node, PREFIX_IA2_STATE, ia2_states);

    // Keyboard shortcut: write either the BSTR contents or empty
    // string, mirroring the C++ if/else.
    let shortcut: Vec<u16> =
        match unsafe { pacc_msaa.get_accKeyboardShortcut(&varchild) } {
            Ok(b) => b.as_wide().to_vec(),
            Err(_) => Vec::new(),
        };
    unsafe {
        parent_node
            .as_field_node()
            .add_attribute(ATTR_KEYBOARD_SHORTCUT, &shortcut);
    }

    // isBlock determination. Order matches the C++ if/else chain.
    let is_block = compute_is_block_element(ia2_states, attribs, role);
    unsafe {
        parent_node.as_field_node().set_is_block(is_block);
    }

    // Force-hide focusable presentation roles. The C++ uses a regex
    // against the vbuf attributes string; we already have the IA2
    // attribs map, so go directly.
    let is_hidden_initial = (states & STATE_SYSTEM_FOCUSABLE) != 0
        && xml_roles_contains_word(attribs, "presentation");
    if is_hidden_initial {
        unsafe {
            parent_node.as_field_node().set_is_hidden(true);
        }
    }

    // Name (kept for blocks 6 / 7).
    let name = match unsafe { pacc_msaa.get_accName(&varchild) } {
        Ok(b) => {
            if is_bstr_null(&b) {
                None
            } else {
                Some(b.as_wide().to_vec())
            }
        }
        Err(_) => None,
    };

    // Description -- written immediately as an attribute when present;
    // also kept around for the descriptionIsContent check in block 7.
    let description = get_acc_description_native(pacc, 0);
    if let Some(ref d) = description {
        unsafe {
            parent_node.as_field_node().add_attribute(ATTR_DESCRIPTION, d);
        }
    }

    // Locale: assemble lang[-country] from the IA2Locale BSTRs. Variant
    // is fetched (so its Drop runs SysFreeString) but ignored.
    let mut locale: Vec<u16> = Vec::new();
    if let Ok((language, country, _variant)) = unsafe { pacc.get_locale() } {
        if !is_bstr_null(&language) {
            locale.extend_from_slice(language.as_wide());
        }
        if !is_bstr_null(&country) && !locale.is_empty() {
            locale.push(b'-' as u16);
            locale.extend_from_slice(country.as_wide());
        }
        // The three BSTRs drop here -- their Drop runs SysFreeString.
    }

    // Derived booleans -- straight from gecko_ia2.cpp:615-627.
    let is_editable = (role == ROLE_SYSTEM_TEXT
        && ((states & STATE_SYSTEM_FOCUSABLE) != 0
            || (states & STATE_SYSTEM_UNAVAILABLE) != 0))
        || (ia2_states & IA2_STATE_EDITABLE) != 0;
    let in_link = (states & STATE_SYSTEM_LINKED) != 0;
    let is_root = id == ctx.root_id;
    let is_embedded_app = role == IA2_ROLE_EMBEDDED_OBJECT
        || (!is_root
            && (role == ROLE_SYSTEM_APPLICATION || role == ROLE_SYSTEM_DIALOG));
    let is_never_interactive = is_hidden_initial
        || (!is_editable
            && (is_root
                || role == ROLE_SYSTEM_DOCUMENT
                || role == IA2_ROLE_INTERNAL_FRAME));
    let is_interactive = !is_never_interactive
        && (is_editable
            || in_link
            || (states & STATE_SYSTEM_FOCUSABLE) != 0
            || (states & STATE_SYSTEM_UNAVAILABLE) != 0
            || is_embedded_app
            || role == ROLE_SYSTEM_EQUATION);

    Block2State {
        name,
        description,
        locale,
        states,
        ia2_states,
        is_editable,
        in_link,
        is_root,
        is_embedded_app,
        is_never_interactive,
        is_interactive,
    }
}

/// Helper: write `IAccessible::state_<bit>` (or
/// `IAccessible2::state_<bit>`) attribute pairs for every set bit in
/// `states`.
fn write_state_attributes(
    node: VbufControlFieldNode,
    prefix: &[u16],
    states: i32,
) {
    if states == 0 {
        return;
    }
    for i in 0..32i32 {
        let state_bit: i32 = 1i32.wrapping_shl(i as u32);
        if (state_bit & states) == 0 {
            continue;
        }
        let mut name = prefix.to_vec();
        let mut digit_buf = String::new();
        use core::fmt::Write;
        let _ = write!(digit_buf, "{state_bit}");
        name.extend(digit_buf.encode_utf16());
        unsafe {
            node.as_field_node().add_attribute(&name, VAL_ONE);
        }
    }
}

/// Port of the isBlockElement decision tree at gecko_ia2.cpp:564-580.
fn compute_is_block_element(
    ia2_states: i32,
    attribs: &BTreeMap<String, String>,
    role: i32,
) -> bool {
    if (ia2_states & IA2_STATE_MULTI_LINE) != 0 {
        // Multiline nodes are always block.
        return true;
    }
    if let Some(display) = attribs.get("display") {
        // The display attribute is authoritative when present.
        return display != "inline"
            && display != "inline-block"
            && display != "inline-flex";
    }
    if attribs.get("formatting").map(|v| v.as_str()) == Some("block") {
        return true;
    }
    matches!(
        role,
        ROLE_SYSTEM_TABLE
            | ROLE_SYSTEM_CELL
            | IA2_ROLE_SECTION
            | ROLE_SYSTEM_DOCUMENT
            | IA2_ROLE_INTERNAL_FRAME
            | IA2_ROLE_UNKNOWN
            | ROLE_SYSTEM_SEPARATOR
    )
}

/// Word-boundary substring check on the `xml-roles` IA2 attribute,
/// equivalent to the wregex `\b<word>\b` used at gecko_ia2.cpp:584.
/// Tokens are separated by any non-`[A-Za-z0-9_]` char.
fn xml_roles_contains_word(
    attribs: &BTreeMap<String, String>,
    word: &str,
) -> bool {
    let value = match attribs.get("xml-roles") {
        Some(v) => v,
        None => return false,
    };
    value
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|tok| tok == word)
}

/// VT_I4 raw vt code, per OAIDL.h.
const VT_I4_RAW: u16 = 3;

/// Apply each `(key, val)` pair from the IA2 attributes map onto the
/// vbuf control field node as an `IAccessible2::attribute_<key>`
/// attribute. Mirrors the C++ loop at gecko_ia2.cpp:474-479.
fn apply_ia2_attribs_to_node(
    node: VbufControlFieldNode,
    attribs: &BTreeMap<String, String>,
) {
    for (key, val) in attribs.iter() {
        let key_u16: Vec<u16> = key.encode_utf16().collect();
        let mut name =
            Vec::with_capacity(PREFIX_IA2_ATTRIBUTE.len() + key_u16.len());
        name.extend_from_slice(PREFIX_IA2_ATTRIBUTE);
        name.extend_from_slice(&key_u16);
        let val_u16: Vec<u16> = val.encode_utf16().collect();
        unsafe {
            node.as_field_node().add_attribute(&name, &val_u16);
        }
    }
}

/// Port of `hasXmlRoleAttribContainingValue` from gecko_ia2.cpp:36.
/// Returns `true` if the IA2 attribute `xml-roles` is present and
/// contains `value` as a substring.
pub(crate) fn has_xml_role_attrib_containing_value(
    attribs: &BTreeMap<String, String>,
    value: &str,
) -> bool {
    attribs
        .get("xml-roles")
        .map(|v| v.contains(value))
        .unwrap_or(false)
}

/// Port of `hasAriaHiddenAttribute` from gecko_ia2.cpp.
/// Returns `true` if the IA2 attribute `hidden` is present and equals
/// `"true"` (case-sensitive, as the C++ original).
pub(crate) fn has_aria_hidden_attribute(
    attribs: &BTreeMap<String, String>,
) -> bool {
    attribs.get("hidden").map(|v| v.as_str()) == Some("true")
}

// ----- block 3 ------------------------------------------------------

/// State produced by [`block3`] and consumed by later blocks.
/// Mirrors the locals declared in gecko_ia2.cpp:630-755.
pub struct Block3State {
    pub is_aria_hidden: bool,
    pub child_count: i32,
    pub is_img_map: bool,
    pub name_is_explicit: bool,
    pub name_is_content: bool,
    /// `true` when the explicit accessible name is sourced from a
    /// label that visibly appears elsewhere in the tree. Used by block
    /// 7 to decide whether to set `alwaysReportName`.
    pub label_visible: bool,
    /// IA2 unique ID of the labelling element, when present.
    /// Consumed by block 7's `labelledByContent` detection.
    pub label_id: Option<i32>,
    /// Captured `IAccessibleText` interface (when `pacc` exposes one).
    /// Lives until end of fillVBuf so the segmentation loop can reuse
    /// it. The Drop runs `Release`.
    pub paccText: Option<IAccessibleText>,
    /// The full text returned by `IAccessibleText::get_text(0, -1)`,
    /// or `None` if the QI failed or the call returned NULL.
    pub ia2_text: Option<Vec<u16>>,
    /// `true` when the captured text is purely whitespace and the
    /// node is not editable. Used to suppress child rendering.
    pub ia2_text_is_unneeded_space: bool,
    pub is_visible: bool,
    pub render_children: bool,
    pub render_selected_item_only: bool,
}

/// Block 3 of `fillVBuf`: aria-hidden / childCount, name flags
/// (nameIsExplicit / nameIsContent / labelVisible),
/// `IAccessibleText` capture, render-flag derivation, and the
/// `IAccessibleAction` loop. Mirrors lines 628-755 of gecko_ia2.cpp.
///
/// Mutates `block2.is_interactive` per the action-name rule.
///
/// # Safety
///
/// `pacc` must be live for the duration; `parent_node` must be a
/// live control field node owned by the buffer.
pub unsafe fn block3(
    pacc: &IAccessible2,
    parent_node: VbufControlFieldNode,
    role: i32,
    attribs: &BTreeMap<String, String>,
    block2: &mut Block2State,
) -> Block3State {
    let is_aria_hidden = has_aria_hidden_attribute(attribs);
    let child_count = get_child_count_native(pacc, is_aria_hidden);
    let is_img_map = role == ROLE_SYSTEM_GRAPHIC && child_count > 0;

    let name_is_explicit =
        attribs.get("explicit-name").map(|v| v.as_str()) == Some("true");

    // Lazily-fetched label info. We only need it for the
    // checkbox/radio name-is-content rule and (later) for explicit-
    // name handling. Fetching unconditionally would cost a COM call
    // per non-checkbox/radio, non-explicitly-named node; gate on the
    // conditions that consult it.
    let label_info: Option<LabelInfo> = if role == ROLE_SYSTEM_CHECKBUTTON
        || role == ROLE_SYSTEM_RADIOBUTTON
        || name_is_explicit
    {
        unsafe { get_label_info_native(pacc) }
    } else {
        None
    };
    let is_label_visible =
        label_info.as_ref().map(|i| i.is_visible).unwrap_or(false);
    let label_id = label_info.as_ref().and_then(|i| i.id);

    let name_is_content = block2.is_embedded_app
        || role == ROLE_SYSTEM_LINK
        || role == ROLE_SYSTEM_PUSHBUTTON
        || role == IA2_ROLE_TOGGLE_BUTTON
        || role == ROLE_SYSTEM_MENUITEM
        || (role == ROLE_SYSTEM_GRAPHIC && !is_img_map)
        || (role == ROLE_SYSTEM_TEXT && !block2.is_editable)
        || role == IA2_ROLE_HEADING
        || role == ROLE_SYSTEM_PAGETAB
        || role == ROLE_SYSTEM_BUTTONMENU
        || ((role == ROLE_SYSTEM_CHECKBUTTON
            || role == ROLE_SYSTEM_RADIOBUTTON)
            && !is_label_visible);

    // labelVisible is the C++ local — used in block 4 (table summary)
    // and block 7 (alwaysReportName / labelledByContent). The C++
    // checks `name && name[0]` (BSTR is non-NULL and not empty).
    let name_present_nonempty = block2
        .name
        .as_ref()
        .map(|n| !n.is_empty())
        .unwrap_or(false);
    let label_visible = name_is_explicit
        && name_present_nonempty
        && (!name_is_content || role == ROLE_SYSTEM_TABLE)
        && is_label_visible;

    // alwaysReportName attribute: explicit name not used as content
    // and not visible elsewhere (e.g. aria-label on an edit field),
    // excluding tables (their summary handling is bespoke).
    if name_is_explicit
        && !name_is_content
        && role != ROLE_SYSTEM_TABLE
        && !label_visible
    {
        unsafe {
            parent_node
                .as_field_node()
                .add_attribute(ATTR_ALWAYS_REPORT_NAME, VAL_TRUE);
        }
    }

    // Capture IAccessibleText + its full text.
    let paccText: Option<IAccessibleText> = pacc.cast().ok();
    let ia2_text: Option<Vec<u16>> = paccText.as_ref().and_then(|t| {
        // `IA2_TEXT_OFFSET_LENGTH = -1` per AccessibleText.idl.
        let bstr = unsafe { t.get_text(0, -1) }.ok()?;
        if is_bstr_null(&bstr) {
            None
        } else {
            Some(bstr.as_wide().to_vec())
        }
    });
    let ia2_text_length = ia2_text.as_ref().map(|t| t.len()).unwrap_or(0);
    let ia2_text_is_unneeded_space = if ia2_text_length > 0
        && !block2.is_editable
    {
        // Mirrors the C++ scan: bail at the first '\n', embedded
        // object char (\xfffc), or any non-whitespace character.
        ia2_text.as_ref().is_some_and(|t| {
            t.iter().all(|&c| {
                c != b'\n' as u16
                    && c != EMBEDDED_OBJ_CHAR
                    && is_whitespace_w(c)
            })
        })
    } else {
        false
    };

    // Render flags.
    let mut is_visible = true;
    let mut render_children = true;
    let mut render_selected_item_only = false;
    if is_aria_hidden {
        is_visible = false;
    } else {
        // Render only the selected item for interactive lists and for
        // outlines (treegrids in the C++ original use the same
        // shortcut).
        let is_interactive_list = role == ROLE_SYSTEM_LIST
            && (block2.states & STATE_SYSTEM_READONLY) == 0;
        if is_interactive_list || role == ROLE_SYSTEM_OUTLINE {
            render_selected_item_only = true;
        }
        if ia2_text_is_unneeded_space
            || role == ROLE_SYSTEM_COMBOBOX
            || render_selected_item_only
            || block2.is_embedded_app
            || role == ROLE_SYSTEM_EQUATION
            || (name_is_content && name_is_explicit)
        {
            render_children = false;
        }
    }

    // IAccessibleAction loop. Writes IAccessibleAction_<name>=<i>
    // attribs and may upgrade is_interactive on click / showlongdesc.
    if let Ok(paccAction) = pacc.cast::<IAccessibleAction>() {
        let n_actions = unsafe { paccAction.nActions() }.unwrap_or(0);
        for i in 0..n_actions {
            let action_name_bstr = match unsafe { paccAction.get_name(i) } {
                Ok(b) => b,
                Err(_) => continue,
            };
            if is_bstr_null(&action_name_bstr) {
                continue;
            }
            let action_name = action_name_bstr.as_wide();

            let mut attr_name: Vec<u16> = PREFIX_IA_ACTION.to_vec();
            attr_name.extend_from_slice(action_name);
            let mut idx_buf = String::new();
            use core::fmt::Write;
            let _ = write!(idx_buf, "{i}");
            let idx_u16: Vec<u16> = idx_buf.encode_utf16().collect();
            unsafe {
                parent_node
                    .as_field_node()
                    .add_attribute(&attr_name, &idx_u16);
            }

            if !block2.is_never_interactive
                && (slice_eq(action_name, "click")
                    || slice_eq(action_name, "showlongdesc"))
            {
                block2.is_interactive = true;
            }
        }
    }

    Block3State {
        is_aria_hidden,
        child_count,
        is_img_map,
        name_is_explicit,
        name_is_content,
        label_visible,
        label_id,
        paccText,
        ia2_text,
        ia2_text_is_unneeded_space,
        is_visible,
        render_children,
        render_selected_item_only,
    }
}

/// Embedded object character used by IAccessibleText to mark
/// hyperlink positions. From AccessibleText.idl.
const EMBEDDED_OBJ_CHAR: u16 = 0xfffc;

/// Whitespace check on a UTF-16 code unit. Matches the C++ `iswspace`
/// for the BMP characters fillVBuf encounters: space, tab, CR, LF, FF,
/// VT, plus the other Unicode "space" code points iswspace recognizes
/// (NBSP at U+00A0, line separator U+2028, paragraph separator
/// U+2029, etc.). Char's `is_whitespace` covers these.
fn is_whitespace_w(c: u16) -> bool {
    char::from_u32(c as u32)
        .map(|ch| ch.is_whitespace())
        .unwrap_or(false)
}

/// `wcscmp(slice, ascii) == 0` for a UTF-16 slice and an ASCII
/// literal. Returns true when the slice's wide-char content matches
/// the literal byte-for-byte (treating each ASCII byte as a u16).
fn slice_eq(slice: &[u16], ascii: &str) -> bool {
    if slice.len() != ascii.len() {
        return false;
    }
    slice
        .iter()
        .zip(ascii.bytes())
        .all(|(s, a)| *s == a as u16)
}

// ----- block 4 ------------------------------------------------------

/// Per-element state produced by [`block4`].
pub struct Block4State {
    /// Owns the `IAccessibleTable2` we QI'd for this node, if any. The
    /// caller (fill_vbuf) borrows from this for child recursion when
    /// `propagation` is [`TablePropagation::Promoted`]. Even when not
    /// promoted, holding the value here keeps the AddRef alive until
    /// fill_vbuf's frame ends.
    pub cur_node_pacc_table2: Option<IAccessibleTable2>,
    /// Decision for child-recursion's `pacc_table2`.
    pub propagation: TablePropagation,
    /// Updated `tableID` for child recursion. Reset to 0 inside a
    /// cell, replaced by `id` on a table-root, otherwise inherited
    /// from the caller.
    pub table_id: i32,
    /// `parentPresentationalRowNumber` for child recursion. `Some`
    /// when this node had a `rowindex` IA2 attribute; otherwise the
    /// caller's value is propagated unchanged (which fill_vbuf
    /// handles).
    pub presentational_row_number: Option<Vec<u16>>,
    /// `IAccessible::accValue` BSTR contents. `None` when the COM
    /// call failed, returned NULL, or returned an empty string
    /// (mirroring the C++ post-process at gecko_ia2.cpp:898).
    pub value: Option<Vec<u16>>,
    /// Possibly-updated `previousNode` for child recursion. Equal to
    /// the input when block 4 didn't add a summary text node.
    pub previous_node: Option<VbufFieldNode>,
}

/// How block 4's table-state mutation propagates to child recursion.
pub enum TablePropagation {
    /// No change: child recursion should see the caller's `pacc_table2`.
    Inherit,
    /// Cell case: descendants are no longer in a table.
    Clear,
    /// New table-root: borrow [`Block4State::cur_node_pacc_table2`]
    /// for children.
    Promoted,
}

/// Block 4 of `fillVBuf`: table-cell-info, parent-table flags, the
/// new-table QI with its `table-rowcount` / `table-columncount` /
/// summary text node, presentational row/col attributes, accValue,
/// and the `alwaysRerenderDescendants` flag for `nameIsContent`.
/// Mirrors lines 757-908 of gecko_ia2.cpp.
///
/// # Safety
///
/// All pointer-borrowing args (`pacc`, `parent_node`, `pacc_table2`,
/// `previous_node`) must point at live objects.
#[allow(clippy::too_many_arguments)]
pub unsafe fn block4(
    pacc: &IAccessible2,
    parent_node: VbufControlFieldNode,
    role: i32,
    id: i32,
    attribs: &BTreeMap<String, String>,
    block2: &Block2State,
    block3: &Block3State,
    pacc_table2_in: Option<&IAccessibleTable2>,
    table_id_in: i32,
    parent_pres_row_num: Option<&[u16]>,
    buffer: VbufBuffer,
    previous_node_in: Option<VbufFieldNode>,
) -> Block4State {
    let mut table_id = table_id_in;
    let mut previous_node = previous_node_in;

    // Step 1: parent-table flags. The C++ first marks denyReuse on
    // any node within a tracked parent table, then adds the row /
    // row-group propagation flags only when this node itself is not
    // a cell.
    let pacc_table_cell: Option<IAccessibleTableCell> = pacc.cast().ok();
    let in_parent_table = pacc_table2_in.is_some();
    if in_parent_table {
        unsafe { parent_node.set_deny_reuse_if_previous_siblings_changed(true) };
        if pacc_table_cell.is_none() {
            // Just rows and row groups (anything table-like that's
            // not the cell itself).
            if role != ROLE_SYSTEM_ROW {
                unsafe { parent_node.set_requires_parent_update(true) };
            }
            unsafe { parent_node.set_always_rerender_children(true) };
        }
    }

    // Step 2: cell case. Fill cell info; recompute table_id from the
    // cell when there's no parent table tracked (update render);
    // write `table-id`; then clear paccTable2 / tableID for descendants.
    let mut cell_cleared = false;
    if let Some(cell) = &pacc_table_cell {
        unsafe { fill_table_cell_info_native(parent_node.as_field_node(), cell) };
        if pacc_table2_in.is_none() {
            // No parent table tracked -- this is an update render.
            table_id = get_table_id_from_cell(cell).unwrap_or(0);
        }
        write_int_attribute_on(
            parent_node.as_field_node(),
            ATTR_TABLE_ID,
            table_id,
        );
        cell_cleared = true;
        table_id = 0;
    }
    drop(pacc_table_cell);

    // Active pacc_table2 after the cell branch. The new-table QI in
    // step 3 only runs if this is None.
    let table2_active_after_cell: Option<&IAccessibleTable2> = if cell_cleared
    {
        None
    } else {
        pacc_table2_in
    };

    // Step 3: new-table QI. Only when we're not already inside a
    // tracked table.
    let mut cur_node_pacc_table2: Option<IAccessibleTable2> = None;
    let mut promoted = false;
    if table2_active_after_cell.is_none() {
        if let Ok(table) = pacc.cast::<IAccessibleTable2>() {
            // layout-guess heuristic.
            if attribs.contains_key("layout-guess") {
                unsafe {
                    parent_node
                        .as_field_node()
                        .add_attribute(ATTR_TABLE_LAYOUT, VAL_ONE);
                }
            }
            // table-id = this node's IA2 unique ID.
            table_id = id;
            write_int_attribute_on(
                parent_node.as_field_node(),
                ATTR_TABLE_ID,
                id,
            );

            // Row/col counts. C++ only writes the attribute if the
            // call succeeded; the > 0 propagation check uses 0 as the
            // failed-call default.
            let row_count = unsafe { table.get_nRows() }.ok();
            if let Some(rc) = row_count {
                write_int_attribute_on(
                    parent_node.as_field_node(),
                    ATTR_TABLE_ROWCOUNT,
                    rc,
                );
            }
            let col_count = unsafe { table.get_nColumns() }.ok();
            if let Some(cc) = col_count {
                write_int_attribute_on(
                    parent_node.as_field_node(),
                    ATTR_TABLE_COLUMNCOUNT,
                    cc,
                );
            }
            promoted =
                row_count.unwrap_or(0) > 0 || col_count.unwrap_or(0) > 0;

            // Summary text node: description if visible & non-empty,
            // else the name when there's no visible label-elsewhere.
            let summary: Option<&[u16]> = if block3.is_visible
                && block2
                    .description
                    .as_ref()
                    .map(|d| !d.is_empty())
                    .unwrap_or(false)
            {
                block2.description.as_deref()
            } else if block2
                .name
                .as_ref()
                .map(|n| !n.is_empty())
                .unwrap_or(false)
                && !block3.label_visible
            {
                block2.name.as_deref()
            } else {
                None
            };
            if let Some(summary_text) = summary {
                if let Some(text_node) = unsafe {
                    buffer.add_text_field_node(
                        Some(parent_node),
                        previous_node,
                        summary_text,
                    )
                } {
                    if !block2.locale.is_empty() {
                        unsafe {
                            text_node
                                .add_attribute(ATTR_LANGUAGE, &block2.locale);
                        }
                    }
                    previous_node = Some(text_node);
                }
            }

            cur_node_pacc_table2 = Some(table);
        }
    }

    // Step 4: parentPresentationalRowNumber forwarding. The C++ uses
    // the C-string null-terminated parent value; we use a slice.
    if let Some(ppres) = parent_pres_row_num {
        unsafe {
            parent_node
                .as_field_node()
                .add_attribute(ATTR_TABLE_ROWNUMBER_PRES, ppres);
        }
    }

    // Step 5: presentational row/col index/count attributes from IA2
    // attribs map. `rowindex` is also propagated to children via
    // presentational_row_number.
    let mut presentational_row_number: Option<Vec<u16>> = None;
    if let Some(rowindex) = attribs.get("rowindex") {
        let value_u16: Vec<u16> = rowindex.encode_utf16().collect();
        unsafe {
            parent_node
                .as_field_node()
                .add_attribute(ATTR_TABLE_ROWNUMBER_PRES, &value_u16);
        }
        presentational_row_number = Some(value_u16);
    }
    for (key, attr_name) in [
        ("colindex", ATTR_TABLE_COLUMNNUMBER_PRES),
        ("rowcount", ATTR_TABLE_ROWCOUNT_PRES),
        ("colcount", ATTR_TABLE_COLUMNCOUNT_PRES),
    ] {
        if let Some(val) = attribs.get(key) {
            let value_u16: Vec<u16> = val.encode_utf16().collect();
            unsafe {
                parent_node
                    .as_field_node()
                    .add_attribute(attr_name, &value_u16);
            }
        }
    }

    // Step 6: accValue. For links, store IAccessible::value attribute.
    // Treat empty BSTR as absent (mirrors gecko_ia2.cpp:898).
    let pacc_msaa: &IAccessible = pacc;
    let varchild = VARIANT::from(0i32);
    let value: Option<Vec<u16>> =
        match unsafe { pacc_msaa.get_accValue(&varchild) } {
            Ok(b) if !is_bstr_null(&b) => {
                if role == ROLE_SYSTEM_LINK {
                    unsafe {
                        parent_node
                            .as_field_node()
                            .add_attribute(ATTR_IA_VALUE, b.as_wide());
                    }
                }
                let wide = b.as_wide();
                if wide.is_empty() {
                    None
                } else {
                    Some(wide.to_vec())
                }
            }
            _ => None,
        };

    // Step 7: alwaysRerenderDescendants when nameIsContent.
    if block3.name_is_content {
        unsafe { parent_node.set_always_rerender_descendants(true) };
    }

    let propagation = if promoted {
        TablePropagation::Promoted
    } else if cell_cleared {
        TablePropagation::Clear
    } else {
        TablePropagation::Inherit
    };

    Block4State {
        cur_node_pacc_table2,
        propagation,
        table_id,
        presentational_row_number,
        value,
        previous_node,
    }
}

/// Helper: write a decimal integer attribute on a vbuf field node.
fn write_int_attribute_on(node: VbufFieldNode, name: &[u16], value: i32) {
    let mut buf = String::new();
    use core::fmt::Write;
    let _ = write!(buf, "{value}");
    let value_u16: Vec<u16> = buf.encode_utf16().collect();
    unsafe {
        node.add_attribute(name, &value_u16);
    }
}

// ----- block 5 ------------------------------------------------------

/// Outcome of [`block5`]. `handled = true` means one of the four
/// content branches fired and block 6 should not run its role-
/// specific branches.
pub struct Block5Outcome {
    pub previous_node: Option<VbufFieldNode>,
    pub ignore_interactive_unlabelled_graphics: bool,
    pub handled: bool,
}

/// Block 5 of `fillVBuf`: image-map name, ignoreInteractiveUnlabelled-
/// Graphics propagation, and the recursive content if/else
/// (text-segmentation loop, AccessibleChildren walk,
/// renderSelectedItemOnly, COMBOBOX). Mirrors lines 911-1130 of
/// gecko_ia2.cpp.
///
/// Mutates `block2.is_interactive` per the graphic alt="" rule (set
/// by block 6's GRAPHIC branch, but block 5 forwards
/// ignore_interactive_unlabelled_graphics for its own children).
///
/// # Safety
///
/// All borrowed pointer args must point at live objects.
#[allow(clippy::too_many_arguments)]
pub unsafe fn block5(
    pacc: &IAccessible2,
    parent_node: VbufControlFieldNode,
    role: i32,
    doc_handle: i32,
    id: i32,
    attribs: &BTreeMap<String, String>,
    block2: &Block2State,
    block3: &Block3State,
    block4: &Block4State,
    buffer: VbufBuffer,
    pacc_table2_for_children: Option<&IAccessibleTable2>,
    pres_row_for_children: Option<&[u16]>,
    mut previous_node: Option<VbufFieldNode>,
    mut ignore_interactive_unlabelled_graphics: bool,
    ctx: &FillVBufCtx,
) -> Block5Outcome {
    // Image map name pre-render (line 911-915 of gecko_ia2.cpp).
    if block3.is_img_map {
        if let Some(name) = block2.name.as_deref() {
            previous_node = add_text_node_with_locale(
                buffer,
                parent_node,
                previous_node,
                name,
                &block2.locale,
            );
        }
    }

    // Propagate ignoreInteractiveUnlabelledGraphics into descendants.
    // (Line 917-921.) Once we have a name, descendants don't need
    // alt-derived names.
    if block2.is_interactive && !ignore_interactive_unlabelled_graphics {
        ignore_interactive_unlabelled_graphics = block2.name.is_some();
    }

    let render_children = block3.render_children;
    let ia2_text_length = block3
        .ia2_text
        .as_ref()
        .map(|t| t.len() as i32)
        .unwrap_or(0);

    let mut handled = false;
    let handles = TextHandleInfo { doc_handle, id };
    let parent_text_align: Option<&str> =
        attribs.get("text-align").map(|v| v.as_str());

    // Branch 1: text segmentation loop (line 923).
    if render_children && ia2_text_length > 0 {
        handled = true;
        // SAFETY: paccText is Some when ia2_text was captured (block 3).
        let pacc_text = block3.paccText.as_ref().expect("paccText present");
        let ia2_text = block3.ia2_text.as_ref().expect("ia2_text present");

        // Lazily build a HyperlinkGetter (mirrors C++
        // makeHyperlinkGetter). Prefer hypertext2.
        let mut link_getter = make_hyperlink_getter(pacc);

        let mut chunk_start: i32 = 0;
        let mut attribs_end: i32 = 0;
        let mut text_attribs: BTreeMap<String, String> = BTreeMap::new();

        let mut i: i32 = 0;
        loop {
            let at_text_end = i == ia2_text_length;
            let at_attribs_end = i == attribs_end;
            let at_obj_char = (i as usize) < ia2_text.len()
                && ia2_text[i as usize] == EMBEDDED_OBJ_CHAR;

            // Flush pending chunk at chunk boundary. The C++ original
            // does this *before* re-fetching attribs, so the chunk's
            // text node gets the *outgoing* run's text_attribs.
            if i != chunk_start
                && (at_text_end || at_attribs_end || at_obj_char)
            {
                let chunk_text =
                    &ia2_text[chunk_start as usize..i as usize];
                if let Some(text_node) = unsafe {
                    buffer.add_text_field_node(
                        Some(parent_node),
                        previous_node,
                        chunk_text,
                    )
                } {
                    previous_node = Some(text_node);
                    add_ia2_text_attribs(text_node, chunk_start, handles);
                    apply_text_segment_attribs(
                        text_node,
                        &text_attribs,
                        parent_text_align,
                    );
                }
            }
            if at_text_end {
                break;
            }
            if at_attribs_end {
                // Start of a new attributes run.
                text_attribs.clear();
                chunk_start = i;
                match unsafe { pacc_text.get_attributes(attribs_end) } {
                    Ok((_s, e, attribs_bstr)) => {
                        attribs_end = e;
                        if !is_bstr_null(&attribs_bstr) {
                            let s = attribs_bstr.to_string();
                            text_attribs = parse_attribs(&s);
                        }
                    }
                    Err(_) => {
                        attribs_end = ia2_text_length;
                    }
                }
            }
            if at_obj_char {
                chunk_start = i + 1;
                if let Some(getter) = link_getter.as_mut() {
                    let link = unsafe { getter.next() };
                    if let Some(link) = link {
                        if let Ok(child_pacc) = link.cast::<IAccessible2>() {
                            let child_node = unsafe {
                                fill_vbuf(
                                    &child_pacc,
                                    buffer,
                                    Some(parent_node),
                                    previous_node,
                                    pacc_table2_for_children,
                                    block4.table_id,
                                    pres_row_for_children,
                                    ignore_interactive_unlabelled_graphics,
                                    ctx,
                                )
                            };
                            if let Some(node) = child_node {
                                previous_node = Some(node);
                                add_ia2_text_attribs(node, i, handles);
                            }
                        }
                    }
                }
            }
            i += 1;
        }
        drop(link_getter);
    } else if render_children && block3.child_count > 0 {
        handled = true;
        // Branch 2: AccessibleChildren walk (line 1019-1080).
        let pacc_msaa: &IAccessible = pacc;
        let mut variants: Vec<VARIANT> =
            vec![VARIANT::default(); block3.child_count as usize];
        let mut filled: i32 = 0;
        let res = unsafe {
            AccessibleChildren(
                pacc_msaa,
                0,
                &mut variants[..],
                &mut filled,
            )
        };
        if res.is_ok() {
            variants.truncate(filled as usize);
        } else {
            variants.clear();
        }

        for child in variants.iter() {
            let raw = child.as_raw();
            let vt = unsafe { raw.Anonymous.Anonymous.vt };
            // VT_DISPATCH = 9 per OAIDL.h.
            if vt != 9 {
                continue;
            }
            let pdisp =
                unsafe { raw.Anonymous.Anonymous.Anonymous.pdispVal };
            if pdisp.is_null() {
                continue;
            }
            let pdisp_raw: *mut core::ffi::c_void = pdisp as *mut _;
            let pdisp_ref: &IDispatch =
                match IDispatch::from_raw_borrowed(&pdisp_raw) {
                    Some(p) => p,
                    None => continue,
                };
            let child_pacc = match pdisp_ref.cast::<IAccessible2>() {
                Ok(a) => a,
                Err(_) => continue,
            };
            let child_node = unsafe {
                fill_vbuf(
                    &child_pacc,
                    buffer,
                    Some(parent_node),
                    previous_node,
                    pacc_table2_for_children,
                    block4.table_id,
                    pres_row_for_children,
                    ignore_interactive_unlabelled_graphics,
                    ctx,
                )
            };
            if let Some(node) = child_node {
                previous_node = Some(node);
            }
        }
        // Each VARIANT drops at Vec drop, running VariantClear and
        // releasing the cached pdispVal references.
    } else if block3.render_selected_item_only {
        handled = true;
        // Branch 3: getSelectedItem -> recurse (line 1081-1104).
        if let Some(item) = unsafe { get_selected_item(pacc) } {
            let child_node = unsafe {
                fill_vbuf(
                    &item,
                    buffer,
                    Some(parent_node),
                    previous_node,
                    pacc_table2_for_children,
                    block4.table_id,
                    pres_row_for_children,
                    ignore_interactive_unlabelled_graphics,
                    ctx,
                )
            };
            if let Some(node) = child_node {
                previous_node = Some(node);
                // Treat the returned field node as a control field
                // node and force requiresParentUpdate. The C++ does
                // the same static_cast (gecko_ia2.cpp:1100); the
                // reuse path is the only case where this would be a
                // misleading downcast, but the field is laid out
                // such that the write goes to a harmless slot.
                unsafe {
                    VbufControlFieldNode(node.0)
                        .set_requires_parent_update(true);
                }
            }
        }
    } else if role == ROLE_SYSTEM_COMBOBOX {
        handled = true;
        // Branch 4: ARIA 1.1 combobox text-box child, or fall back
        // to the value text (line 1106-1130).
        if let Some(text_box) = unsafe { get_text_box_in_combo_box(pacc) } {
            let child_node = unsafe {
                fill_vbuf(
                    &text_box,
                    buffer,
                    Some(parent_node),
                    previous_node,
                    pacc_table2_for_children,
                    block4.table_id,
                    pres_row_for_children,
                    ignore_interactive_unlabelled_graphics,
                    ctx,
                )
            };
            if let Some(node) = child_node {
                previous_node = Some(node);
            }
        } else if let Some(value) = block4.value.as_deref() {
            previous_node = add_text_node_with_locale(
                buffer,
                parent_node,
                previous_node,
                value,
                &block2.locale,
            );
        }
    }

    Block5Outcome {
        previous_node,
        ignore_interactive_unlabelled_graphics,
        handled,
    }
}

/// Add a text field node with an optional language attribute when
/// `locale` is non-empty. Returns the new node (or `None` on failure).
fn add_text_node_with_locale(
    buffer: VbufBuffer,
    parent: VbufControlFieldNode,
    previous: Option<VbufFieldNode>,
    text: &[u16],
    locale: &[u16],
) -> Option<VbufFieldNode> {
    let node =
        unsafe { buffer.add_text_field_node(Some(parent), previous, text) }?;
    if !locale.is_empty() {
        unsafe {
            node.add_attribute(ATTR_LANGUAGE, locale);
        }
    }
    Some(node)
}

/// Apply the per-segment text attributes (the `text_attribs` map plus
/// any IA2 `text-align` override on the parent) onto a just-flushed
/// chunk's text node.
fn apply_text_segment_attribs(
    node: VbufFieldNode,
    text_attribs: &BTreeMap<String, String>,
    text_align: Option<&str>,
) {
    for (k, v) in text_attribs {
        let k_u16: Vec<u16> = k.encode_utf16().collect();
        let v_u16: Vec<u16> = v.encode_utf16().collect();
        unsafe {
            node.add_attribute(&k_u16, &v_u16);
        }
    }
    if let Some(ta) = text_align {
        let value: Vec<u16> = ta.encode_utf16().collect();
        unsafe {
            node.add_attribute(ATTR_TEXT_ALIGN, &value);
        }
    }
}

/// Helper for the common case of writing the IA2 text-offset trio
/// (ia2TextStartOffset / ia2TextWindowHandle / ia2TextUniqueID) onto
/// a text node.
fn add_ia2_text_attribs(
    node: VbufFieldNode,
    offset: i32,
    handles: TextHandleInfo,
) {
    write_int_attribute_on(node, ATTR_IA2_TEXT_START_OFFSET, offset);
    write_int_attribute_on(
        node,
        ATTR_IA2_TEXT_WINDOW_HANDLE,
        handles.doc_handle,
    );
    write_int_attribute_on(node, ATTR_IA2_TEXT_UNIQUE_ID, handles.id);
}

#[derive(Clone, Copy)]
struct TextHandleInfo {
    doc_handle: i32,
    id: i32,
}

/// Construct a `HyperlinkGetter` Rust-internally, mirroring
/// `makeHyperlinkGetter` in `nvdaHelper/common/ia2utils.cpp`. Returns
/// `None` when neither `IAccessibleHypertext2` nor
/// `IAccessibleHypertext` is supported.
fn make_hyperlink_getter(pacc: &IAccessible2) -> Option<HyperlinkGetter> {
    if let Ok(ht2) = pacc.cast::<IAccessibleHypertext2>() {
        return Some(HyperlinkGetter::Ht2 {
            hypertext: ht2,
            links: None,
            index: 0,
        });
    }
    if let Ok(ht) = pacc.cast::<IAccessibleHypertext>() {
        return Some(HyperlinkGetter::Ht {
            hypertext: ht,
            index: 0,
        });
    }
    None
}

// ----- block 6 ------------------------------------------------------

/// Outcome of [`block6`]: the (possibly mutated) `previous_node`.
pub struct Block6Outcome {
    pub previous_node: Option<VbufFieldNode>,
}

/// Block 6 of `fillVBuf`: the GRAPHIC / PROGRESSBAR / value
/// non-recursive content branches (run only when block 5 didn't
/// fire), and the trailing always-fires fallbacks for nodes with no
/// useful content. Mirrors lines 1131-1202 of gecko_ia2.cpp.
///
/// Mutates `block2.is_interactive` in the GRAPHIC alt="" branch.
///
/// # Safety
///
/// `parent_node` must be a live control field node; `buffer` live.
#[allow(clippy::too_many_arguments)]
pub unsafe fn block6(
    parent_node: VbufControlFieldNode,
    role: i32,
    attribs: &BTreeMap<String, String>,
    block2: &mut Block2State,
    block3: &Block3State,
    block4: &Block4State,
    buffer: VbufBuffer,
    block5_handled: bool,
    ignore_interactive_unlabelled_graphics: bool,
    mut previous_node: Option<VbufFieldNode>,
) -> Block6Outcome {
    // Branches that continue block 5's else-if chain. Only run when
    // block 5 didn't already fire.
    if !block5_handled {
        if role == ROLE_SYSTEM_GRAPHIC {
            // Graphic name handling.
            let name = block2.name.as_deref();
            if name.map(|n| !n.is_empty()).unwrap_or(false) {
                // Has a non-empty label.
                previous_node = add_text_node_with_locale(
                    buffer,
                    parent_node,
                    previous_node,
                    name.unwrap(),
                    &block2.locale,
                );
            } else if (name.map(|n| n.is_empty()).unwrap_or(false))
                || ignore_interactive_unlabelled_graphics
            {
                // alt="" or we've decided to ignore unlabelled graphics.
                block2.is_interactive = false;
            } else if block2.is_interactive {
                // Unlabelled but interactive -- derive a label from the
                // link URL or the graphic's src attribute.
                if block2.in_link {
                    if let Some(value) = block4.value.as_deref() {
                        let derived = get_name_for_url(value);
                        if !derived.is_empty() {
                            previous_node = add_text_node_with_locale(
                                buffer,
                                parent_node,
                                previous_node,
                                &derived,
                                &block2.locale,
                            );
                        }
                    }
                } else if let Some(src) = attribs.get("src") {
                    let src_u16: Vec<u16> = src.encode_utf16().collect();
                    let derived = get_name_for_url(&src_u16);
                    if !derived.is_empty() {
                        previous_node = add_text_node_with_locale(
                            buffer,
                            parent_node,
                            previous_node,
                            &derived,
                            &block2.locale,
                        );
                    }
                }
            }
        } else if role == ROLE_SYSTEM_PROGRESSBAR
            && (block2.states & STATE_SYSTEM_INDETERMINATE) != 0
        {
            // Indeterminate progress bar -> render a single space.
            previous_node = add_text_node_with_locale(
                buffer,
                parent_node,
                previous_node,
                EMPTY_TEXT_NODE,
                &block2.locale,
            );
        } else if !block3.name_is_content {
            if let Some(value) = block4.value.as_deref() {
                previous_node = add_text_node_with_locale(
                    buffer,
                    parent_node,
                    previous_node,
                    value,
                    &block2.locale,
                );
            }
        }
    }

    // ----- trailing fallbacks (always run inside isVisible) -----

    // Fallback 1: useful-content rescue. If the node has no useful
    // content and its name can serve as content, render the name (or
    // derive from URL for links).
    let needs_content_rescue = !block2.is_editable
        && (block3.name_is_content
            || role == IA2_ROLE_SECTION
            || role == IA2_ROLE_TEXT_FRAME)
        && !unsafe { parent_node.as_field_node().has_useful_content() };
    if needs_content_rescue {
        if let Some(name) = block2.name.as_deref() {
            // C++ passes NULL for `previous` here.
            if let Some(text_node) = unsafe {
                buffer.add_text_field_node(Some(parent_node), None, name)
            } {
                if !block2.locale.is_empty() {
                    unsafe {
                        text_node.add_attribute(ATTR_LANGUAGE, &block2.locale)
                    };
                }
            }
        } else if role == ROLE_SYSTEM_LINK {
            if let Some(value) = block4.value.as_deref() {
                let derived = get_name_for_url(value);
                if !derived.is_empty() {
                    // C++ doesn't capture the resulting node.
                    let _ = unsafe {
                        buffer.add_text_field_node(
                            Some(parent_node),
                            None,
                            &derived,
                        )
                    };
                }
            }
        }
    }

    // Fallback 2: empty cells / unknown roles get a single space and
    // are forced to inline (isBlock = false).
    let is_table_cell_role = role == ROLE_SYSTEM_CELL
        || role == ROLE_SYSTEM_ROWHEADER
        || role == ROLE_SYSTEM_COLUMNHEADER
        || role == IA2_ROLE_UNKNOWN;
    let parent_length = unsafe { parent_node.as_field_node().get_length() };
    if is_table_cell_role && parent_length == 0 {
        previous_node = add_text_node_with_locale(
            buffer,
            parent_node,
            previous_node,
            EMPTY_TEXT_NODE,
            &block2.locale,
        );
        unsafe { parent_node.as_field_node().set_is_block(false) };
    }

    // Fallback 3: interactive / nameable / described nodes that
    // would otherwise be empty also get a single space, so the user
    // can land on them.
    let parent_length_2 = unsafe { parent_node.as_field_node().get_length() };
    let has_name = block2.name.as_ref().map(|n| !n.is_empty()).unwrap_or(false);
    let has_description = block2
        .description
        .as_ref()
        .map(|d| !d.is_empty())
        .unwrap_or(false);
    let needs_empty_filler = (block2.is_interactive
        || role == ROLE_SYSTEM_SEPARATOR
        || has_name
        || has_description)
        && parent_length_2 == 0;
    if needs_empty_filler {
        previous_node = add_text_node_with_locale(
            buffer,
            parent_node,
            previous_node,
            EMPTY_TEXT_NODE,
            &block2.locale,
        );
    }

    Block6Outcome { previous_node }
}

// ----- block 7 ------------------------------------------------------

/// Block 7 of `fillVBuf`: name-as-attribute (with the
/// `labelledByContent` detection), `descriptionIsContent` flag, and
/// the calls to `fill_vbuf_aria_details` / `fill_vbuf_aria_error`.
/// Mirrors lines 1206-1245 of gecko_ia2.cpp.
///
/// # Safety
///
/// `pacc` must be live for the duration; `parent_node` must be a live
/// control field node owned by the buffer.
#[allow(clippy::too_many_arguments)]
pub unsafe fn block7(
    pacc: &IAccessible2,
    parent_node: VbufControlFieldNode,
    doc_handle: i32,
    role_attr: &[u16],
    block2: &Block2State,
    block3: &Block3State,
    buffer: VbufBuffer,
    ctx: &FillVBufCtx,
) {
    // Name attribute. C++ checks `if (name)` (BSTR non-NULL); we
    // mirror via Option::is_some, which is `Some` even for empty
    // strings -- matches the C++ behavior of writing an empty name
    // attribute when the server returned a non-null empty BSTR.
    if !block3.name_is_content {
        if let Some(name) = block2.name.as_deref() {
            unsafe {
                parent_node.as_field_node().add_attribute(ATTR_NAME, name);
            }
            // labelledByContent: only relevant when the name was
            // explicit (browser-supplied label), and only when the
            // labelling element itself is a descendant of this node.
            if block3.name_is_explicit {
                if let Some(label_id) = block3.label_id {
                    if let Some(label_node) = unsafe {
                        buffer.get_control_field_node_with_identifier(
                            doc_handle, label_id,
                        )
                    } {
                        let is_descendant = unsafe {
                            buffer.is_descendant_node(
                                parent_node.as_field_node(),
                                label_node.as_field_node(),
                            )
                        };
                        if is_descendant {
                            unsafe {
                                parent_node.as_field_node().add_attribute(
                                    ATTR_LABELLED_BY_CONTENT,
                                    VAL_TRUE,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // descriptionIsContent: prevent NVDA from announcing a description
    // that's literally the same as the rendered text.
    if let Some(description) = block2.description.as_deref() {
        let matches = unsafe {
            parent_node.as_field_node().content_matches_string(description)
        };
        if matches {
            unsafe {
                parent_node
                    .as_field_node()
                    .add_attribute(ATTR_DESCRIPTION_IS_CONTENT, VAL_TRUE);
            }
        }
    }

    // aria-details / aria-errormessage relation handling.
    unsafe {
        fill_vbuf_aria_details_native(
            doc_handle,
            pacc,
            buffer,
            parent_node,
            role_attr,
            ctx.is_chrome,
        );
        fill_vbuf_aria_error_native(
            pacc,
            parent_node.as_field_node(),
            ctx.is_chrome,
        );
    }
}

// ----- entry point ---------------------------------------------------

/// Per-render context the recursion threads through. Carries the
/// pieces of `GeckoVBufBackend_t` state that fillVBuf consults beyond
/// its formal arguments.
///
/// Constructed once at the C++ entry point, then borrowed by every
/// recursive Rust frame.
pub struct FillVBufCtx {
    /// The vbuf backend handle. Equivalent to `this` in the C++
    /// original; used to drive the render-thread machinery
    /// (`force_update`, `request_update`, root identifiers).
    pub backend: VbufBackend,
    /// The backend's live ("main") Rust `storage::Buffer` handle.
    /// Cross-buffer reuse queries this directly (the C++ backend is not
    /// a Rust `Buffer`). Set once by the update orchestration from
    /// `state.buffer`; a partial re-render's temp buffer is a
    /// *different* allocation from `main`.
    pub main: VbufBuffer,
    /// The IA2 unique ID of the document root node. Used for the
    /// `isRoot` check (gecko_ia2.cpp:620). Equivalent to
    /// `this->rootID`.
    pub root_id: i32,
    /// `true` when the toolkit name is `"Chrome"`. Threaded down to
    /// `fill_vbuf_aria_details` / `fill_vbuf_aria_error` for the
    /// Chrome-specific `IAccessible2_2::get_relationTargetsOfType`
    /// workaround. Equivalent to `this->toolkitName == L"Chrome"`.
    pub is_chrome: bool,
}

/// Rust-internal entry point. Recursion uses this directly; the
/// extern C shim ([`nvda_ia2_fill_vbuf`]) constructs the inputs from
/// raw pointers and forwards.
///
/// `parent_pres_row_num` mirrors `parentPresentationalRowNumber` in
/// the C++ original — `None` means the parent does not propagate a
/// presentational row number; `Some(slice)` carries the wide-char
/// digits.
///
/// # Safety
///
/// * `pacc` must be live for the duration of the call.
/// * `buffer`, `parent`, `previous`, and `ctx.backend` must point at
///   live vbuf nodes / backends.
/// * `pacc_table2`, when `Some`, must be live for the duration.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn fill_vbuf(
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    parent: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    pacc_table2: Option<&IAccessibleTable2>,
    table_id: i32,
    parent_pres_row_num: Option<&[u16]>,
    ignore_interactive_unlabelled_graphics: bool,
    ctx: &FillVBufCtx,
) -> Option<VbufFieldNode> {
    // Block 1.
    let cont = match unsafe {
        block1(pacc, buffer, ctx, parent, previous)
    } {
        Block1Outcome::Bail => return None,
        Block1Outcome::Reused(reference) => return Some(reference),
        Block1Outcome::Continue(c) => c,
    };

    // C++ `fillVBuf` resets `previousNode = NULL` immediately after
    // creating the new control field node (gecko_ia2.cpp:468). Every
    // node rendered from here on (the table summary in block 4, the
    // text segments / children in block 5, the fallback content in
    // block 6, aria-details in block 7) is a child of the NEW control
    // node `cont.parent_node`, so the incoming *sibling* `previous`
    // must NOT carry into those blocks: it belongs to this node's
    // parent, not to this node. Threading it onward makes the storage
    // (correctly) reject each such insert -- `previous.parent` is the
    // grandparent, not `cont.parent_node` -- which silently drops the
    // text of every non-first child, leaving control-field structure
    // but no body text. Shadow the parameter to `None` to reproduce the
    // C++ reset for all subsequent blocks.
    let previous: Option<VbufFieldNode> = None;

    // Block 2.
    let mut block2_state = unsafe {
        block2(pacc, cont.parent_node, cont.role, &cont.attribs, cont.id, ctx)
    };

    // Block 3.
    let block3_state = unsafe {
        block3(
            pacc,
            cont.parent_node,
            cont.role,
            &cont.attribs,
            &mut block2_state,
        )
    };

    // Block 4. Owns cur_node_pacc_table2 for the rest of fill_vbuf so
    // the borrow used by child recursion stays alive.
    let block4_state = unsafe {
        block4(
            pacc,
            cont.parent_node,
            cont.role,
            cont.id,
            &cont.attribs,
            &block2_state,
            &block3_state,
            pacc_table2,
            table_id,
            parent_pres_row_num,
            buffer,
            previous,
        )
    };
    let pacc_table2_for_children: Option<&IAccessibleTable2> =
        match block4_state.propagation {
            TablePropagation::Inherit => pacc_table2,
            TablePropagation::Clear => None,
            TablePropagation::Promoted => {
                block4_state.cur_node_pacc_table2.as_ref()
            }
        };
    let pres_row_for_children: Option<&[u16]> = block4_state
        .presentational_row_number
        .as_deref()
        .or(parent_pres_row_num);

    // Blocks 5 + 6 only run when isVisible. The render-flag is on
    // block 3.
    if block3_state.is_visible {
        // Block 5 — content rendering (recursive).
        let block5_outcome = unsafe {
            block5(
                pacc,
                cont.parent_node,
                cont.role,
                cont.doc_handle,
                cont.id,
                &cont.attribs,
                &block2_state,
                &block3_state,
                &block4_state,
                buffer,
                pacc_table2_for_children,
                pres_row_for_children,
                block4_state.previous_node,
                ignore_interactive_unlabelled_graphics,
                ctx,
            )
        };

        // Block 6 — non-recursive content branches + trailing fallbacks.
        let _block6_outcome = unsafe {
            block6(
                cont.parent_node,
                cont.role,
                &cont.attribs,
                &mut block2_state,
                &block3_state,
                &block4_state,
                buffer,
                block5_outcome.handled,
                block5_outcome.ignore_interactive_unlabelled_graphics,
                block5_outcome.previous_node,
            )
        };
    }

    // Block 7 — name attribute / labelledByContent /
    // descriptionIsContent / aria-details / aria-errormessage. Runs
    // unconditionally (outside the isVisible guard).
    unsafe {
        block7(
            pacc,
            cont.parent_node,
            cont.doc_handle,
            &cont.role_attr,
            &block2_state,
            &block3_state,
            buffer,
            ctx,
        );
    }

    Some(cont.parent_node.as_field_node())
}

// NOTE (Phase 6e, Stage D): the former `nvda_ia2_fill_vbuf` extern "C"
// entry point was removed at the flip. Its only C++ caller was
// `GeckoVBufBackend_t::fillVBuf` (itself only reached from the old
// `render()`), and gecko's `render()` is now a vestigial stub -- the
// live path drives [`fill_vbuf`] directly from the Rust update
// orchestration ([`crate::gecko_backend_state::nvda_ia2_gecko_backend_update`]),
// which sets `FillVBufCtx.main` from `state.buffer`. With no remaining
// caller (verified by grep), leaving the extern would be dead FFI
// surface, so it and its C++ declaration were deleted together.

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn xml_roles_substring_match() {
        let m = map_of(&[("xml-roles", "treegrid checkbox")]);
        assert!(has_xml_role_attrib_containing_value(&m, "treegrid"));
        assert!(has_xml_role_attrib_containing_value(&m, "checkbox"));
        // The C++ uses wstring::find -- a literal substring search,
        // so "tree" matches "treegrid" and "absent" does not.
        assert!(has_xml_role_attrib_containing_value(&m, "tree"));
        assert!(!has_xml_role_attrib_containing_value(&m, "absent"));
    }

    #[test]
    fn xml_roles_missing_attribute() {
        let m = map_of(&[]);
        assert!(!has_xml_role_attrib_containing_value(&m, "treegrid"));
    }

    #[test]
    fn aria_hidden_true() {
        let m = map_of(&[("hidden", "true")]);
        assert!(has_aria_hidden_attribute(&m));
    }

    #[test]
    fn aria_hidden_false_or_missing() {
        let m = map_of(&[("hidden", "false")]);
        assert!(!has_aria_hidden_attribute(&m));
        let empty = map_of(&[]);
        assert!(!has_aria_hidden_attribute(&empty));
    }

    #[test]
    fn is_block_multiline_state_wins() {
        let m = map_of(&[("display", "inline")]);
        // IA2_STATE_MULTI_LINE forces block even if display says inline.
        assert!(compute_is_block_element(IA2_STATE_MULTI_LINE, &m, 0));
    }

    #[test]
    fn is_block_display_attribute_authoritative() {
        let m_block = map_of(&[("display", "block")]);
        assert!(compute_is_block_element(0, &m_block, 0));
        for inline in ["inline", "inline-block", "inline-flex"] {
            let m = map_of(&[("display", inline)]);
            assert!(!compute_is_block_element(0, &m, 0));
        }
    }

    #[test]
    fn is_block_formatting_block() {
        let m = map_of(&[("formatting", "block")]);
        // role doesn't matter when formatting=block matches.
        assert!(compute_is_block_element(0, &m, ROLE_SYSTEM_TEXT));
        let m_other = map_of(&[("formatting", "inline")]);
        // formatting != block => fall through to role-based default.
        // ROLE_SYSTEM_TEXT is *not* in the block-by-default set.
        assert!(!compute_is_block_element(0, &m_other, ROLE_SYSTEM_TEXT));
    }

    #[test]
    fn is_block_role_fallback() {
        let empty = map_of(&[]);
        // Block-by-default roles.
        for role in [
            ROLE_SYSTEM_TABLE,
            ROLE_SYSTEM_CELL,
            IA2_ROLE_SECTION,
            ROLE_SYSTEM_DOCUMENT,
            IA2_ROLE_INTERNAL_FRAME,
            IA2_ROLE_UNKNOWN,
            ROLE_SYSTEM_SEPARATOR,
        ] {
            assert!(compute_is_block_element(0, &empty, role), "role {role:#x}");
        }
        // Other roles fall through to inline.
        for role in [ROLE_SYSTEM_TEXT, ROLE_SYSTEM_GRAPHIC] {
            assert!(
                !compute_is_block_element(0, &empty, role),
                "role {role:#x}"
            );
        }
    }

    #[test]
    fn xml_roles_word_boundary_basic() {
        let m = map_of(&[("xml-roles", "presentation")]);
        assert!(xml_roles_contains_word(&m, "presentation"));

        let m_with_others = map_of(&[("xml-roles", "menubar presentation")]);
        assert!(xml_roles_contains_word(&m_with_others, "presentation"));
        assert!(xml_roles_contains_word(&m_with_others, "menubar"));
    }

    #[test]
    fn xml_roles_word_boundary_partial_no_match() {
        // Substring of a longer token must not match.
        let m = map_of(&[("xml-roles", "representation")]);
        assert!(!xml_roles_contains_word(&m, "presentation"));
    }

    #[test]
    fn xml_roles_word_boundary_missing_attr() {
        let m = map_of(&[]);
        assert!(!xml_roles_contains_word(&m, "presentation"));
    }

    #[test]
    fn slice_eq_matches_ascii_only() {
        let click: Vec<u16> = "click".encode_utf16().collect();
        assert!(slice_eq(&click, "click"));
        assert!(!slice_eq(&click, "clic"));
        assert!(!slice_eq(&click, "clicks"));

        let empty: [u16; 0] = [];
        assert!(slice_eq(&empty, ""));
    }

    #[test]
    fn whitespace_check_handles_common_cases() {
        for c in [' ', '\t', '\r', '\n', '\u{00a0}', '\u{2028}'] {
            assert!(is_whitespace_w(c as u16), "{c:?}");
        }
        for c in ['a', '0', '_', '\u{fffc}'] {
            assert!(!is_whitespace_w(c as u16), "{c:?}");
        }
    }
}
