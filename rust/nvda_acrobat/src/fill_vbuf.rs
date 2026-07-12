//! Rust port of `AdobeAcrobatVBufBackend_t::fillVBuf` + `renderText` +
//! the table helpers from
//! `nvdaHelper/vbufBackends/adobeAcrobat/adobeAcrobat.cpp`.
//!
//! Renders a PDF accessible subtree (MSAA `IAccessible` + Acrobat
//! `IPDDom*`) into a Rust `nvda_vbuf::storage::Buffer` via the
//! [`VbufBuffer`] wrapper. Structure mirrors the C++ line-for-line; see
//! the referenced line numbers throughout.
//!
//! Two deliberate departures from the C++, both behaviour-preserving for
//! text output (see `docs/plans/2026-07-12-rust-vbuf-acrobat-port.md`):
//!
//! * The custom node's `language` field is threaded down the recursion as
//!   `inherited_lang` and, for the cross-render update case, persisted on
//!   each control node as the `acrobat::language` attribute (read back by
//!   the backend adapter to seed a re-rendered subtree's root).
//! * `parentNode->getPrevious()` (C++ line 688) is the incoming
//!   `previous` argument (the new control node is inserted immediately
//!   after it), so we save it rather than add a sibling accessor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IServiceProvider;
use windows::Win32::UI::Accessibility::{
    AccessibleChildren, IAccessible, WindowFromAccessibleObject,
};

use nvda_vbuf::{VbufBuffer, VbufControlFieldNode, VbufFieldNode};

use crate::interfaces::{
    IGetPDDomNode, IPDDomDocPagination, IPDDomElement, IPDDomNode,
    IPDDomNodeExt, IAccID, SID_ACC_ID, SID_GET_PDDOM_NODE,
};

// --- Constants ------------------------------------------------------------

// MSAA roles (oleacc.h).
const ROLE_SYSTEM_TABLE: i32 = 0x18;
const ROLE_SYSTEM_COLUMNHEADER: i32 = 0x19;
const ROLE_SYSTEM_ROWHEADER: i32 = 0x1a;
const ROLE_SYSTEM_ROW: i32 = 0x1c;
const ROLE_SYSTEM_CELL: i32 = 0x1d;
const ROLE_SYSTEM_LINK: i32 = 0x1e;
const ROLE_SYSTEM_LIST: i32 = 0x21;
const ROLE_SYSTEM_GRAPHIC: i32 = 0x28;
const ROLE_SYSTEM_PUSHBUTTON: i32 = 0x2b;
const ROLE_SYSTEM_CHECKBUTTON: i32 = 0x2c;
const ROLE_SYSTEM_RADIOBUTTON: i32 = 0x2d;
const ROLE_SYSTEM_COMBOBOX: i32 = 0x2e;

// MSAA states (oleacc.h).
const STATE_SYSTEM_FOCUSABLE: i32 = 0x0010_0000;

// CPDDomNodeType (AcrobatAccess.idl).
const CPDDOMNODE_WORD: i32 = 5;

// FontInfoState (AcrobatAccess.idl).
const FONTINFO_NOINFO: i32 = 2;
const FONTINFO_MIXEDINFO: i32 = 3;
const FONTINFO_VALID: i32 = 4;

// PDDOM_FontStyle (AcrobatAccess.idl).
const PDDOM_FONTATTR_ITALIC: i32 = 0x1;
const PDDOM_FONTATTR_BOLD: i32 = 0x10;

// adobeAcrobat.cpp text flags.
const TEXTFLAG_UNDERLINE: i32 = 0x1;
const TEXTFLAG_STRIKETHROUGH: i32 = 0x2;

// adobeAcrobat.cpp table-header types.
const TABLEHEADER_COLUMN: i32 = 0x1;
const TABLEHEADER_ROW: i32 = 0x2;

// VARIANT vt values (OAIDL.h).
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;

const CR: u16 = b'\r' as u16;
const LF: u16 = b'\n' as u16;

/// Per-render context, built by the backend adapter once per `render()`.
pub struct FillVBufCtx {
    /// The Win32 identifier pair's doc handle (constant per document).
    pub doc_handle: i32,
    /// Whether the document is an XFA form (enables the
    /// `WindowFromAccessibleObject` workaround for child nodes).
    pub is_xfa: bool,
    /// The document's pagination interface, if any, for page labels.
    pub doc_pagination: Option<IPDDomDocPagination>,
}

// --- Small helpers --------------------------------------------------------

/// UTF-16 for a string literal (attribute names/values).
#[inline]
pub(crate) fn u(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// `Some(bstr)` when the call succeeded and returned a non-empty string;
/// `None` otherwise (mirrors the C++ `nullifyEmpty` + `if (!text)` idiom).
#[inline]
fn ok_nonempty(r: windows::core::Result<BSTR>) -> Option<BSTR> {
    match r {
        Ok(b) if !b.is_empty() => Some(b),
        _ => None,
    }
}

/// Read a PDF element attribute, treating absent/empty as `None`.
/// `owner` is the attribute owner (`None` -> null `BSTR`, matching the
/// C++ `NULL`).
fn get_attr(
    de: &IPDDomElement,
    attr: &str,
    owner: Option<&str>,
) -> Option<BSTR> {
    let attr_b = BSTR::from(attr);
    let owner_b = owner.map(BSTR::from).unwrap_or_default();
    ok_nonempty(unsafe { de.get_attribute(&attr_b, &owner_b) })
}

/// The `IServiceProvider` for `pacc`, or `None`.
pub(crate) fn service_provider(pacc: &IAccessible) -> Option<IServiceProvider> {
    pacc.cast().ok()
}

/// Port of C++ `getAccID`: `IServiceProvider` -> `IAccID` -> id, or 0.
fn get_acc_id(servprov: &IServiceProvider) -> i32 {
    let paccid: IAccID = match unsafe { servprov.QueryService(&SID_ACC_ID) } {
        Ok(p) => p,
        Err(_) => return 0,
    };
    unsafe { paccid.get_acc_id() }.unwrap_or(0)
}

/// Port of C++ `getPDDomNode`: `IServiceProvider` -> `IGetPDDomNode` ->
/// `get_PDDomNode(varChild)`.
pub(crate) fn get_pddom_node(
    varchild: &VARIANT,
    servprov: &IServiceProvider,
) -> Option<IPDDomNode> {
    let pget: IGetPDDomNode =
        unsafe { servprov.QueryService(&SID_GET_PDDOM_NODE) }.ok()?;
    unsafe { pget.get_pddom_node(varchild) }.ok()
}

/// Port of C++ `AdobeAcrobatVBufBackend_t::getPageNum`. Returns the page
/// label (or 1-based page number) when the node spans a single page.
fn get_page_num(dom_node: &IPDDomNode, ctx: &FillVBufCtx) -> Option<Vec<u16>> {
    let ext: IPDDomNodeExt = dom_node.cast().ok()?;
    let (first, last) = unsafe { ext.get_page_num() }.ok()?;
    // Only useful if the first and last pages are the same.
    if first != last {
        return None;
    }
    if let Some(pag) = &ctx.doc_pagination {
        if let Ok(label) = unsafe { pag.label_for_page_num(first) } {
            return Some(label.as_wide().to_vec());
        }
    }
    // GetPageNum is 0-based; present 1-based.
    Some(u(&(first + 1).to_string()))
}

/// Port of C++ `processText`: strip CR/LF.
fn process_text(in_text: &[u16]) -> Vec<u16> {
    in_text
        .iter()
        .copied()
        .filter(|&c| c != CR && c != LF)
        .collect()
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

// --- Table state ----------------------------------------------------------

struct TableHeaderInfo {
    unique_id: i32,
    kind: i32,
}

#[derive(Default)]
struct TableInfo {
    table_id: i32,
    cur_row_number: i32,
    cur_column_number: i32,
    /// Column number -> remaining row span.
    column_row_spans: HashMap<i32, i32>,
    /// Column number -> accumulated `table-columnheadercells` value.
    column_headers: HashMap<i32, Vec<u16>>,
    /// Row number -> accumulated `table-rowheadercells` value.
    row_headers: HashMap<i32, Vec<u16>>,
    /// Header id string -> info (for explicit `Headers` resolution).
    headers_info: HashMap<Vec<u16>, TableHeaderInfo>,
    /// Cells with an explicit `Headers` attribute, resolved when the
    /// table is finalised: (cell node, Headers attribute value).
    nodes_with_explicit_headers: Vec<(VbufControlFieldNode, Vec<u16>)>,
}

/// Port of C++ `handleColsSpannedByPrevRows`: advance `cur_column_number`
/// past columns still spanned by earlier rows, decrementing spans.
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

/// Port of C++ `fillExplicitTableHeadersForCell`: resolve a cell's
/// explicit `Headers` attribute (form `"[[id id ... ]]"`) into
/// `table-columnheadercells` / `table-rowheadercells` attributes.
fn fill_explicit_table_headers_for_cell(
    cell: VbufControlFieldNode,
    doc_handle: i32,
    headers_attr: &[u16],
    ti: &TableInfo,
) {
    let mut col_headers: Vec<u16> = Vec::new();
    let mut row_headers: Vec<u16> = Vec::new();

    // Ignore the "[[" prefix and the " ]]" suffix.
    if headers_attr.len() < 3 {
        return;
    }
    let last_pos = headers_attr.len() - 3;
    let mut start_pos = 2usize;
    while start_pos < last_pos {
        // Find the space that ends this id.
        let end_pos = match headers_attr[start_pos..]
            .iter()
            .position(|&c| c == b' ' as u16)
        {
            Some(off) => start_pos + off,
            None => break,
        };
        let id = &headers_attr[start_pos..end_pos];
        start_pos = end_pos + 1;
        let info = match ti.headers_info.get(id) {
            Some(i) => i,
            None => continue,
        };
        let entry = u(&format!("{},{};", doc_handle, info.unique_id));
        if info.kind & TABLEHEADER_COLUMN != 0 {
            col_headers.extend_from_slice(&entry);
        }
        if info.kind & TABLEHEADER_ROW != 0 {
            row_headers.extend_from_slice(&entry);
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

/// Add the `language` and `page-number` attributes shared by every text
/// node (C++ `addAttrsToTextNode` macro).
fn add_attrs_to_text_node(
    node: VbufFieldNode,
    node_lang: &[u16],
    page_num: Option<&[u16]>,
) {
    unsafe { node.add_attribute(&u("language"), node_lang) };
    if let Some(pn) = page_num {
        unsafe { node.add_attribute(&u("page-number"), pn) };
    }
}

// --- renderText -----------------------------------------------------------

/// Port of C++ `renderText`. Renders `dom_node`'s text (descending into
/// mixed-font subtrees) as text field nodes under `parent_node`. Returns
/// the last text node added, or `None` when no text was produced.
#[allow(clippy::too_many_arguments)]
unsafe fn render_text(
    buffer: VbufBuffer,
    parent_node: VbufControlFieldNode,
    mut previous: Option<VbufFieldNode>,
    dom_node: &IPDDomNode,
    dom_element: Option<&IPDDomElement>,
    name_is_content: bool,
    lang: &[u16],
    flags: i32,
    page_num: Option<&[u16]>,
) -> Option<VbufFieldNode> {
    // Font info for this node.
    let (mut font_status, font_name, font_size, font_flags) =
        match unsafe { dom_node.get_font_info() } {
            Ok(fi) => (fi.status, Some(fi.name), fi.size, fi.flags),
            Err(_) => (FONTINFO_NOINFO, None, 0.0f32, 0),
        };

    // #2174: Alt / ActualText override any other text content.
    let mut text: Option<BSTR> = None;
    if let Some(de) = dom_element {
        text = get_attr(de, "Alt", None);
        if text.is_none() {
            text = get_attr(de, "ActualText", None);
        }
    }

    let mut child_count = 0i32;
    if text.is_none() {
        child_count = unsafe { dom_node.get_child_count() }.unwrap_or(0);
    }

    // #2175 HACK: Reader >= 10.1 reports NoInfo even for mixed info.
    if font_status == FONTINFO_NOINFO && child_count > 0 {
        // Never descend beneath word nodes (word segments double chars).
        if let Ok(nt) = unsafe { dom_node.get_type() } {
            if nt != CPDDOMNODE_WORD {
                font_status = FONTINFO_MIXEDINFO;
            }
        }
    } else if font_status == FONTINFO_MIXEDINFO && child_count == 0 {
        // HACK: ignore MixedInfo with no children (would render empty).
        font_status = FONTINFO_NOINFO;
    }

    if font_status == FONTINFO_MIXEDINFO {
        // Descend to gather per-font text.
        for child_index in 0..child_count {
            let dom_child = match unsafe { dom_node.get_child(child_index) } {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(tn) = unsafe {
                render_text(
                    buffer,
                    parent_node,
                    previous,
                    &dom_child,
                    None,
                    name_is_content,
                    lang,
                    flags,
                    page_num,
                )
            } {
                previous = Some(tn);
            }
        }
    } else {
        // Leaf: gather this node's own text.
        if text.is_none() {
            if name_is_content {
                // #3640: prefer the name where it is the content.
                text = ok_nonempty(unsafe { dom_node.get_name() });
            }
            if text.is_none() {
                text = ok_nonempty(unsafe { dom_node.get_text_content() });
            }
            if !name_is_content && text.is_none() {
                // GetValue sometimes works when GetTextContent didn't.
                text = ok_nonempty(unsafe { dom_node.get_value() });
            }
        }

        if let Some(t) = text {
            let proc = process_text(t.as_wide());
            previous = unsafe {
                buffer.add_text_field_node(Some(parent_node), previous, &proc)
            };
            if let Some(tn) = previous {
                if font_status == FONTINFO_VALID {
                    let name_w =
                        font_name.as_ref().map(|b| b.as_wide()).unwrap_or(&[]);
                    unsafe { tn.add_attribute(&u("font-name"), name_w) };
                    if font_size > 0.0 {
                        unsafe {
                            tn.add_attribute(
                                &u("font-size"),
                                &u(&format!("{:.1}", font_size)),
                            )
                        };
                    }
                    if font_flags & PDDOM_FONTATTR_ITALIC != 0 {
                        unsafe { tn.add_attribute(&u("italic"), &u("1")) };
                    }
                    if font_flags & PDDOM_FONTATTR_BOLD != 0 {
                        unsafe { tn.add_attribute(&u("bold"), &u("1")) };
                    }
                }
                unsafe { tn.add_attribute(&u("language"), lang) };
                if flags & TEXTFLAG_UNDERLINE != 0 {
                    unsafe { tn.add_attribute(&u("underline"), &u("1")) };
                } else if flags & TEXTFLAG_STRIKETHROUGH != 0 {
                    unsafe { tn.add_attribute(&u("strikethrough"), &u("1")) };
                }
                if let Some(pn) = page_num {
                    unsafe { tn.add_attribute(&u("page-number"), pn) };
                }
            }
        } else {
            // No text to add; communicate this to the caller.
            previous = None;
        }
    }

    previous
}

// --- fillVBuf -------------------------------------------------------------

/// Render an accessible subtree into `buffer`. Crate-facing entry point
/// for the backend adapter; the recursive worker [`fill_vbuf_rec`] owns
/// the table-state threading (its private `TableInfo` never surfaces
/// here). `inherited_lang` seeds the root's language (empty for an
/// initial render; the old node's parent's language for a re-render).
///
/// # Safety
///
/// `pacc` must be a live `IAccessible`; `buffer` and any node handles
/// must be valid for this render.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn fill_vbuf(
    pacc: &IAccessible,
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    inherited_lang: &[u16],
    page_num: Option<&[u16]>,
    ctx: &FillVBufCtx,
) -> Option<VbufControlFieldNode> {
    unsafe {
        fill_vbuf_rec(
            pacc,
            buffer,
            parent_node,
            previous,
            inherited_lang,
            None,
            page_num,
            ctx,
        )
    }
}

/// Recursive worker for [`fill_vbuf`]. Mirrors C++
/// `AdobeAcrobatVBufBackend_t::fillVBuf`. Returns the created control
/// node, or `None` (loop guard, missing service provider, etc.).
#[allow(clippy::too_many_arguments)]
unsafe fn fill_vbuf_rec(
    pacc: &IAccessible,
    buffer: VbufBuffer,
    parent_node: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    inherited_lang: &[u16],
    table_info: Option<Rc<RefCell<TableInfo>>>,
    page_num: Option<&[u16]>,
    ctx: &FillVBufCtx,
) -> Option<VbufControlFieldNode> {
    let doc_handle = ctx.doc_handle;

    // childID VARIANT for the IAccessible calls.
    let varchild = VARIANT::from(0i32);

    let servprov = service_provider(pacc)?;

    // GET ID.
    let id = get_acc_id(&servprov);
    // nhAssert(ID) in C++; a zero id is degenerate but we proceed as the
    // loop guard below and identifier storage will simply use it.

    // Protect against loops: bail if we already know this node.
    if unsafe { buffer.get_control_field_node_with_identifier(doc_handle, id) }
        .is_some()
    {
        return None;
    }

    // Add this node. `old_parent_node` is the parent we descended from
    // (null at a render root); `incoming_previous` is this node's
    // previous sibling (== C++ `parentNode->getPrevious()` after insert).
    let old_parent_node = parent_node;
    let incoming_previous = previous;
    let node = unsafe {
        buffer.add_control_field_node(
            parent_node,
            previous,
            doc_handle,
            id,
            true,
        )
    }?;
    // C++ resets previousNode = NULL right after adding the control node.
    let mut previous: Option<VbufFieldNode> = None;

    // Role (accRole): VT_I4 -> number, VT_BSTR -> string, else 0.
    let mut role = 0i32;
    let role_attr: Vec<u16> = match unsafe { pacc.get_accRole(&varchild) } {
        Ok(v) => {
            let raw = v.as_raw();
            let vt = unsafe { raw.Anonymous.Anonymous.vt };
            if vt == VT_BSTR {
                let p = unsafe { raw.Anonymous.Anonymous.Anonymous.bstrVal };
                if p.is_null() {
                    u("0")
                } else {
                    // VARIANT owns the BSTR; borrow without dropping.
                    let b = core::mem::ManuallyDrop::new(unsafe {
                        BSTR::from_raw(p)
                    });
                    b.as_wide().to_vec()
                }
            } else if vt == VT_I4 {
                role = unsafe { raw.Anonymous.Anonymous.Anonymous.lVal };
                u(&role.to_string())
            } else {
                u("0")
            }
        }
        Err(_) => u("0"),
    };
    unsafe {
        node.as_field_node()
            .add_attribute(&u("IAccessible::role"), &role_attr)
    };

    // States (accState): each set bit becomes an attribute.
    let states = match unsafe { pacc.get_accState(&varchild) } {
        Ok(v) => variant_i4(&v).unwrap_or(0),
        Err(_) => 0,
    };
    for i in 0..32 {
        let state = 1i32 << i;
        if state & states != 0 {
            unsafe {
                node.as_field_node().add_attribute(
                    &u(&format!("IAccessible::state_{}", state)),
                    &u("1"),
                )
            };
        }
    }

    let dom_node = get_pddom_node(&varchild, &servprov);
    let dom_element: Option<IPDDomElement> =
        dom_node.as_ref().and_then(|dn| dn.cast().ok());

    let mut std_name: Option<BSTR> = None;
    let mut text_flags = 0i32;
    let mut render_space = false;

    // node_lang starts empty; set from the Lang attribute if present.
    let mut node_lang: Vec<u16> = Vec::new();

    if let Some(de) = &dom_element {
        // stdName.
        std_name = ok_nonempty(unsafe { de.get_std_name() });
        if let Some(sn) = &std_name {
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("acrobat::stdname"), sn.as_wide())
            };
            let sn_w = sn.as_wide();
            if sn_w == u("Span") || sn_w == u("Link") || sn_w == u("Quote") {
                // Inline element.
                unsafe { node.as_field_node().set_is_block(false) };
            }
            if sn_w == u("Formula") {
                // We want a space so the user can reach formulas, but not
                // their content.
                render_space = true;
            }
        }

        // Language.
        if let Some(lang) = get_attr(de, "Lang", None) {
            node_lang = lang.as_wide().to_vec();
        }

        // Underline / strikethrough.
        if let Some(td) = get_attr(de, "TextDecorationType", Some("Layout")) {
            let td_w = td.as_wide();
            if td_w == u("Underline") {
                text_flags |= TEXTFLAG_UNDERLINE;
            } else if td_w == u("LineThrough") {
                text_flags |= TEXTFLAG_STRIKETHROUGH;
            }
        }
    }

    // Inherit language from the parent when this node has none.
    if node_lang.is_empty() {
        node_lang = inherited_lang.to_vec();
    }
    // Persist the resolved language on the control node so a later
    // cross-render update can seed a re-rendered subtree's root from it
    // (the backend adapter reads it back via `acrobat::language`).
    if !node_lang.is_empty() {
        unsafe {
            node.as_field_node()
                .add_attribute(&u("acrobat::language"), &node_lang)
        };
    }

    // Child count. We don't descend into lists / combo boxes (Acrobat
    // reports a count but the children aren't accessible).
    let mut child_count = 0i32;
    if !render_space
        && role != ROLE_SYSTEM_LIST
        && role != ROLE_SYSTEM_COMBOBOX
    {
        child_count = unsafe { pacc.accChildCount() }.unwrap_or(0);
    }

    // Page number: inherit, else derive from this node.
    let owned_page_num: Option<Vec<u16>> = match page_num {
        Some(_) => None,
        None => dom_node.as_ref().and_then(|dn| get_page_num(dn, ctx)),
    };
    let page_num: Option<&[u16]> =
        page_num.or(owned_page_num.as_deref());

    // Table handling. `cur_table` is the table state threaded to this
    // node + its descendants: a freshly created one for a TABLE, else the
    // inherited one.
    let mut owned_table: Option<Rc<RefCell<TableInfo>>> = None;
    if role == ROLE_SYSTEM_TABLE {
        let mut ti = TableInfo {
            table_id: id,
            ..Default::default()
        };
        ti.cur_row_number = 0;
        ti.cur_column_number = 0;
        unsafe {
            node.as_field_node()
                .add_attribute(&u("table-id"), &u(&id.to_string()))
        };
        // Summary text, if present.
        if let Some(de) = &dom_element {
            if let Some(summary) = get_attr(de, "Summary", Some("Table")) {
                if let Some(tn) = unsafe {
                    buffer.add_text_field_node(
                        Some(node),
                        previous,
                        summary.as_wide(),
                    )
                } {
                    add_attrs_to_text_node(tn, &node_lang, page_num);
                    previous = Some(tn);
                }
            }
        }
        owned_table = Some(Rc::new(RefCell::new(ti)));
    } else if role == ROLE_SYSTEM_ROW {
        if let Some(ti) = &table_info {
            let mut ti = ti.borrow_mut();
            ti.cur_row_number += 1;
            ti.cur_column_number = 0;
        }
    } else if role == ROLE_SYSTEM_CELL
        || role == ROLE_SYSTEM_COLUMNHEADER
        || role == ROLE_SYSTEM_ROWHEADER
    {
        if let Some(ti_rc) = &table_info {
            handle_cell(
                node,
                doc_handle,
                id,
                role,
                dom_element.as_ref(),
                &mut ti_rc.borrow_mut(),
            );
        }
    }

    // The table state to thread to descendants.
    let child_table: Option<Rc<RefCell<TableInfo>>> =
        owned_table.clone().or_else(|| table_info.clone());

    if render_space {
        // Just render a space.
        if let Some(tn) = unsafe {
            buffer.add_text_field_node(Some(node), previous, &u(" "))
        } {
            add_attrs_to_text_node(tn, &node_lang, page_num);
            previous = Some(tn);
        }
    } else if child_count > 0 {
        // Iterate children via AccessibleChildren.
        let mut variants: Vec<VARIANT> =
            vec![VARIANT::default(); child_count as usize];
        let mut filled = 0i32;
        let got = unsafe {
            AccessibleChildren(pacc, 0, &mut variants[..], &mut filled)
        };
        if got.is_ok() {
            variants.truncate(filled as usize);
        } else {
            variants.clear();
        }
        for child in variants.iter() {
            let raw = child.as_raw();
            let vt = unsafe { raw.Anonymous.Anonymous.vt };
            if vt != VT_DISPATCH {
                continue;
            }
            let pdisp = unsafe { raw.Anonymous.Anonymous.Anonymous.pdispVal };
            if pdisp.is_null() {
                continue;
            }
            let pdisp_raw: *mut core::ffi::c_void = pdisp as *mut _;
            let child_pacc: IAccessible = match unsafe {
                windows::Win32::System::Com::IDispatch::from_raw_borrowed(
                    &pdisp_raw,
                )
            }
            .and_then(|d| d.cast().ok())
            {
                Some(a) => a,
                None => continue,
            };
            if ctx.is_xfa {
                // HACK: XFA needs WindowFromAccessibleObject so that
                // AccessibleObjectFromEvent works for this node later.
                let mut hwnd = HWND::default();
                let _ = unsafe {
                    WindowFromAccessibleObject(
                        &child_pacc,
                        Some(&mut hwnd as *mut HWND),
                    )
                };
            }
            if let Some(child_node) = unsafe {
                fill_vbuf_rec(
                    &child_pacc,
                    buffer,
                    Some(node),
                    previous,
                    &node_lang,
                    child_table.clone(),
                    page_num,
                    ctx,
                )
            } {
                previous = Some(child_node.as_field_node());
            }
        }
    } else {
        // Leaf node.
        if !ctx.is_xfa && std_name.is_none() {
            // Non-XFA leaf nodes with no stdName are inline.
            unsafe { node.as_field_node().set_is_block(false) };
        }

        // Name (#3645: test accName for graphics).
        let name: Option<BSTR> = if states & STATE_SYSTEM_FOCUSABLE != 0
            || role == ROLE_SYSTEM_GRAPHIC
        {
            ok_nonempty(unsafe { pacc.get_accName(&varchild) })
        } else {
            None
        };

        let use_name_as_content = role == ROLE_SYSTEM_LINK
            || role == ROLE_SYSTEM_PUSHBUTTON
            || role == ROLE_SYSTEM_RADIOBUTTON
            || role == ROLE_SYSTEM_CHECKBUTTON
            // #3645: accName is meaningful for graphics where GetName
            // might return "mc-ref".
            || (role == ROLE_SYSTEM_GRAPHIC && name.is_some());

        if let Some(nm) = &name {
            if !use_name_as_content {
                unsafe {
                    node.as_field_node()
                        .add_attribute(&u("name"), nm.as_wide())
                };
                // Render the name before this node (the label is often
                // not a separate node). Only when descending from a
                // parent (an update re-render has already rendered it).
                if let Some(op) = old_parent_node {
                    if let Some(tn) = unsafe {
                        buffer.add_text_field_node(
                            Some(op),
                            incoming_previous,
                            nm.as_wide(),
                        )
                    } {
                        add_attrs_to_text_node(tn, &node_lang, page_num);
                    }
                }
            }
        }

        // Text content.
        let text_node = if let Some(dn) = &dom_node {
            unsafe {
                render_text(
                    buffer,
                    node,
                    previous,
                    dn,
                    dom_element.as_ref(),
                    use_name_as_content,
                    &node_lang,
                    text_flags,
                    page_num,
                )
            }
        } else {
            None
        };
        if let Some(tn) = text_node {
            previous = Some(tn);
        }

        if text_node.is_none() && states & STATE_SYSTEM_FOCUSABLE != 0 {
            // Focusable but no text: add a space so it's reachable.
            if let Some(tn) = unsafe {
                buffer.add_text_field_node(Some(node), previous, &u(" "))
            } {
                add_attrs_to_text_node(tn, &node_lang, page_num);
                previous = Some(tn);
            }
        }
    }

    // Finalise tables.
    if (role == ROLE_SYSTEM_CELL
        || role == ROLE_SYSTEM_COLUMNHEADER
        || role == ROLE_SYSTEM_ROWHEADER)
        && unsafe { node.as_field_node().get_length() } == 0
    {
        // Always render a space for empty table cells.
        if let Some(tn) = unsafe {
            buffer.add_text_field_node(Some(node), previous, &u(" "))
        } {
            add_attrs_to_text_node(tn, &node_lang, page_num);
        }
        unsafe { node.as_field_node().set_is_block(false) };
    } else if role == ROLE_SYSTEM_TABLE {
        if let Some(ti_rc) = &owned_table {
            let ti = ti_rc.borrow();
            for (cell, headers_attr) in &ti.nodes_with_explicit_headers {
                fill_explicit_table_headers_for_cell(
                    *cell,
                    doc_handle,
                    headers_attr,
                    &ti,
                );
            }
            unsafe {
                node.as_field_node().add_attribute(
                    &u("table-rowcount"),
                    &u(&ti.cur_row_number.to_string()),
                )
            };
            unsafe {
                node.as_field_node().add_attribute(
                    &u("table-columncount"),
                    &u(&ti.cur_column_number.to_string()),
                )
            };
        }
    }

    Some(node)
}

/// Cell handling for `fillVBuf` (C++ lines 532-618): column/row numbering,
/// spans, and implicit/explicit header wiring.
fn handle_cell(
    node: VbufControlFieldNode,
    doc_handle: i32,
    id: i32,
    role: i32,
    dom_element: Option<&IPDDomElement>,
    ti: &mut TableInfo,
) {
    ti.cur_column_number += 1;
    handle_cols_spanned_by_prev_rows(ti);

    let table_id = ti.table_id;
    let cur_row = ti.cur_row_number;
    let start_col = ti.cur_column_number;
    unsafe {
        node.as_field_node()
            .add_attribute(&u("table-id"), &u(&table_id.to_string()));
        node.as_field_node()
            .add_attribute(&u("table-rownumber"), &u(&cur_row.to_string()));
        node.as_field_node()
            .add_attribute(&u("table-columnnumber"), &u(&start_col.to_string()));
    }

    let explicit_headers =
        dom_element.and_then(|de| get_attr(de, "Headers", Some("Table")));
    if let Some(headers) = explicit_headers {
        // Some referenced nodes might not be rendered yet; resolve later.
        ti.nodes_with_explicit_headers
            .push((node, headers.as_wide().to_vec()));
    } else {
        // Implicit column headers for this cell.
        if let Some(h) = ti.column_headers.get(&start_col) {
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("table-columnheadercells"), h)
            };
        }
        // Implicit row headers for this cell.
        if let Some(h) = ti.row_headers.get(&cur_row) {
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("table-rowheadercells"), h)
            };
        }
    }

    // The last row spanned by this cell (updated below on a row span).
    let mut end_row = cur_row;
    if let Some(de) = dom_element {
        if let Some(colspan) = get_attr(de, "ColSpan", Some("Table")) {
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("table-columnsspanned"), colspan.as_wide())
            };
            let extra = wtoi(colspan.as_wide()) - 1;
            ti.cur_column_number += extra.max(0);
        }
        if let Some(rowspan) = get_attr(de, "RowSpan", Some("Table")) {
            unsafe {
                node.as_field_node()
                    .add_attribute(&u("table-rowsspanned"), rowspan.as_wide())
            };
            let span = wtoi(rowspan.as_wide()) - 1;
            if span > 0 {
                for col in start_col..=ti.cur_column_number {
                    ti.column_row_spans.insert(col, span);
                }
                end_row += span;
            }
        }
    }

    if role == ROLE_SYSTEM_COLUMNHEADER || role == ROLE_SYSTEM_ROWHEADER {
        let mut header_type = 0i32;
        if let Some(de) = dom_element {
            if let Some(scope) = get_attr(de, "Scope", Some("Table")) {
                let s = scope.as_wide();
                if s == u("Column") {
                    header_type = TABLEHEADER_COLUMN;
                } else if s == u("Row") {
                    header_type = TABLEHEADER_ROW;
                } else if s == u("Both") {
                    header_type = TABLEHEADER_COLUMN | TABLEHEADER_ROW;
                }
            }
        }
        if header_type == 0 {
            header_type = if role == ROLE_SYSTEM_COLUMNHEADER {
                TABLEHEADER_COLUMN
            } else {
                TABLEHEADER_ROW
            };
        }
        let entry = u(&format!("{},{};", doc_handle, id));
        if header_type & TABLEHEADER_COLUMN != 0 {
            for col in start_col..=ti.cur_column_number {
                ti.column_headers.entry(col).or_default().extend_from_slice(&entry);
            }
        }
        if header_type & TABLEHEADER_ROW != 0 {
            for row in cur_row..=end_row {
                ti.row_headers.entry(row).or_default().extend_from_slice(&entry);
            }
        }
        if let Some(de) = dom_element {
            if let Some(id_str) = ok_nonempty(unsafe { de.get_id() }) {
                ti.headers_info.insert(
                    id_str.as_wide().to_vec(),
                    TableHeaderInfo {
                        unique_id: id,
                        kind: header_type,
                    },
                );
            }
        }
    }
}

/// Port of C++ `_wtoi`: parse a leading base-10 integer from a UTF-16
/// slice (leading whitespace/sign, stops at the first non-digit),
/// returning 0 when there's no number.
fn wtoi(s: &[u16]) -> i32 {
    let mut i = 0usize;
    while i < s.len() && (s[i] == b' ' as u16 || s[i] == b'\t' as u16) {
        i += 1;
    }
    let mut sign = 1i32;
    if i < s.len() && (s[i] == b'+' as u16 || s[i] == b'-' as u16) {
        if s[i] == b'-' as u16 {
            sign = -1;
        }
        i += 1;
    }
    let mut val = 0i32;
    while i < s.len() {
        let c = s[i];
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            break;
        }
        val = val.wrapping_mul(10).wrapping_add((c - b'0' as u16) as i32);
        i += 1;
    }
    sign * val
}
