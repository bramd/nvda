//! Ports of the IAccessibleTableCell-driven helpers from
//! `nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:119-195`:
//!
//! * `getTableIDFromCell` -- derives the IA2 unique ID of the cell's
//!   parent table.
//! * `fillTableHeaders` -- walks one header axis and writes a
//!   `"<docHandle>,<id>;"`-formatted attribute on the vbuf node.
//! * `fillTableCellInfo_IATable2` -- writes row/column position and
//!   span attributes, then calls `fillTableHeaders` for both axes.

use core::fmt::Write;

use windows::core::{Interface, IUnknown};
use windows::Win32::System::Com::CoTaskMemFree;

use crate::interfaces::{IAccessible2, IAccessibleTableCell};
use nvda_vbuf::VbufFieldNode;

const ATTR_TABLE_ROWNUMBER: &[u16] = &utf16(b"table-rownumber");
const ATTR_TABLE_COLUMNNUMBER: &[u16] = &utf16(b"table-columnnumber");
const ATTR_TABLE_COLUMNSSPANNED: &[u16] = &utf16(b"table-columnsspanned");
const ATTR_TABLE_ROWSSPANNED: &[u16] = &utf16(b"table-rowsspanned");
const ATTR_TABLE_COLUMNHEADERCELLS: &[u16] =
    &utf16(b"table-columnheadercells");
const ATTR_TABLE_ROWHEADERCELLS: &[u16] = &utf16(b"table-rowheadercells");

/// Compile-time ASCII-to-UTF-16 conversion. The byte array must be
/// pure ASCII; non-ASCII bytes are accepted only for `<= 0x7f` and
/// produce identical-valued `u16`s.
const fn utf16<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
}

/// Selects which header axis a `fillTableHeaders` call walks.
#[repr(i32)]
pub enum HeaderAxis {
    Column = 0,
    Row = 1,
}

/// C-callable replacement for `getTableIDFromCell`. Returns 0 if any
/// step in the COM chain fails (matching the C++ behavior).
///
/// # Safety
///
/// `cell` must be a valid `IAccessibleTableCell*` for the duration.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_get_table_id_from_cell(
    cell: *mut core::ffi::c_void,
) -> i32 {
    if cell.is_null() {
        return 0;
    }
    let cell_ref: &IAccessibleTableCell =
        match IAccessibleTableCell::from_raw_borrowed(&cell) {
            Some(c) => c,
            None => return 0,
        };
    get_table_id_from_cell(cell_ref).unwrap_or(0)
}

fn get_table_id_from_cell(cell: &IAccessibleTableCell) -> Option<i32> {
    let unk: IUnknown = unsafe { cell.get_table() }.ok()?;
    let acc: IAccessible2 = unk.cast().ok()?;
    unsafe { acc.get_uniqueID() }.ok()
}

/// C-callable replacement for `fillTableCellInfo_IATable2`. Writes row
/// / column position and span attributes, then calls into
/// `fill_table_headers` for both axes.
///
/// # Safety
///
/// * `node` must be a valid `VBufStorage_controlFieldNode_t*`.
/// * `cell` must be a valid `IAccessibleTableCell*`.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_fill_table_cell_info(
    node: *mut core::ffi::c_void,
    cell: *mut core::ffi::c_void,
) {
    if node.is_null() || cell.is_null() {
        return;
    }
    let cell_ref: &IAccessibleTableCell =
        match IAccessibleTableCell::from_raw_borrowed(&cell) {
            Some(c) => c,
            None => return,
        };
    let field_node = VbufFieldNode(node);

    if let Ok(extents) = unsafe { cell_ref.get_row_column_extents() } {
        write_int_attribute(
            field_node,
            ATTR_TABLE_ROWNUMBER,
            extents.row + 1,
        );
        write_int_attribute(
            field_node,
            ATTR_TABLE_COLUMNNUMBER,
            extents.column + 1,
        );
        if extents.column_extents > 1 {
            write_int_attribute(
                field_node,
                ATTR_TABLE_COLUMNSSPANNED,
                extents.column_extents,
            );
        }
        if extents.row_extents > 1 {
            write_int_attribute(
                field_node,
                ATTR_TABLE_ROWSSPANNED,
                extents.row_extents,
            );
        }
    }

    fill_table_headers(field_node, cell_ref, HeaderAxis::Column);
    fill_table_headers(field_node, cell_ref, HeaderAxis::Row);
}

fn write_int_attribute(node: VbufFieldNode, name: &[u16], value: i32) {
    let mut buf = String::new();
    let _ = write!(buf, "{value}");
    let value_u16: Vec<u16> = buf.encode_utf16().collect();
    unsafe {
        node.add_attribute(name, &value_u16);
    }
}

fn fill_table_headers(
    node: VbufFieldNode,
    cell: &IAccessibleTableCell,
    axis: HeaderAxis,
) {
    let attr_name = match axis {
        HeaderAxis::Column => ATTR_TABLE_COLUMNHEADERCELLS,
        HeaderAxis::Row => ATTR_TABLE_ROWHEADERCELLS,
    };
    let (raw_cells, count) = match axis {
        HeaderAxis::Column => match unsafe { cell.get_column_header_cells() }
        {
            Ok(v) => v,
            Err(_) => return,
        },
        HeaderAxis::Row => match unsafe { cell.get_row_header_cells() } {
            Ok(v) => v,
            Err(_) => return,
        },
    };
    if raw_cells.is_null() || count <= 0 {
        unsafe { CoTaskMemFree(Some(raw_cells as *const _)) };
        return;
    }

    let mut value = String::new();
    for i in 0..count as usize {
        let raw_unk = unsafe { core::ptr::read(raw_cells.add(i)) };
        if raw_unk.is_null() {
            continue;
        }
        let unk: IUnknown = unsafe { IUnknown::from_raw(raw_unk) };
        let acc: IAccessible2 = match unk.cast() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let hwnd = match unsafe { acc.get_windowHandle() } {
            Ok(h) => h,
            Err(_) => continue,
        };
        let id = match unsafe { acc.get_uniqueID() } {
            Ok(i) => i,
            Err(_) => continue,
        };
        // The C++ original uses HandleToUlong(hwnd) -- a 64-to-32-bit
        // truncation for HWNDs on x64 (which always fit; HWND values
        // are 32-bit even on 64-bit Windows). Match the same shape.
        let doc_handle: u32 = (hwnd.0 as usize) as u32;
        let _ = write!(value, "{doc_handle},{id};");
    }
    unsafe { CoTaskMemFree(Some(raw_cells as *const _)) };

    if value.is_empty() {
        return;
    }
    let value_u16: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        node.add_attribute(attr_name, &value_u16);
    }
}
