//! Hand-rolled bindings for the Adobe Acrobat `IPDDom*` COM interfaces the
//! adobeAcrobat vbuf backend consumes. Method orderings and IIDs come
//! verbatim from `miscDeps/include/AcrobatAccess/AcrobatAccess.idl`.
//!
//! Only the methods the backend actually calls (surveyed from
//! `nvdaHelper/vbufBackends/adobeAcrobat/adobeAcrobat.cpp`) are given real
//! signatures; earlier vtable slots we don't use are filled with
//! `unused: usize` placeholders to keep the offsets correct, exactly as
//! `nvda_ia2::interfaces` does for the IA2 interfaces. Trailing slots
//! past the last used method are simply omitted.
//!
//! IID quick reference (verbatim from the IDL):
//! - IAccID:               81f9b44f-ba3a-4f5d-9b51-090c74a9b3a4
//! - IPDDomNode:           5007373a-20d7-458f-9ffb-abc900e3a831  (: IDispatch)
//! - IPDDomNodeExt:        4a894040-247e-4aff-bb08-3489e9905235  (: IUnknown)
//! - IPDDomElement:        198f17ae-b921-4308-9543-288d426a5c2b  (: IPDDomNode)
//! - IGetPDDomNode:        f9f2fe81-f764-4bd0-afa5-5de841ddb625  (: IUnknown)
//! - IPDDomDocPagination:  8e6d1cb7-4dae-4de4-8ed9-15672a5f942f  (: IUnknown)
//!
//! Service IDs used with `IServiceProvider::QueryService` live in
//! [`SID_ACC_ID`] and [`SID_GET_PDDOM_NODE`].
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::mem::ManuallyDrop;

use windows::core::{
    Interface, BSTR, GUID, HRESULT, IUnknown, IUnknown_Vtbl, VARIANT,
};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::System::Com::{IDispatch, IDispatch_Vtbl};

// --- Service IDs (AcrobatAccess.idl `cpp_quote` block) --------------------

/// `SID_AccID` — service id to reach [`IAccID`] via
/// `IServiceProvider::QueryService`.
pub const SID_ACC_ID: GUID = GUID::from_u128(0x449d454b_1f46_497e_b2b6_3357aed9912b);

/// `SID_GetPDDomNode` — service id to reach [`IGetPDDomNode`] via
/// `IServiceProvider::QueryService`.
pub const SID_GET_PDDOM_NODE: GUID =
    GUID::from_u128(0xc0a1d5e9_1142_4cf3_b607_82fc3b96a4df);

// --- Small helpers --------------------------------------------------------

/// Bitwise-copy an `[in]` COM value into a `ManuallyDrop` for a by-value
/// vtable parameter without transferring ownership. The original stays
/// live and is dropped by the caller; the callee treats `[in]` params as
/// borrowed (COM convention), so this copy must NOT run its destructor.
///
/// # Safety
///
/// The returned `ManuallyDrop<T>` aliases `*value` bit-for-bit; the caller
/// must keep `value` alive for the duration of the call and must not let
/// both copies run `T`'s destructor.
#[inline]
unsafe fn in_param<T>(value: &T) -> ManuallyDrop<T> {
    ManuallyDrop::new(unsafe { core::ptr::read(value) })
}

/// Finish a call whose sole `[out, retval]` is a server-allocated `BSTR`.
/// Takes ownership on success (the returned `BSTR` `SysFreeString`s on
/// drop); on failure reclaims any partially-written `BSTR` so it is freed.
#[inline]
unsafe fn bstr_out(
    hr: HRESULT,
    out: ManuallyDrop<BSTR>,
) -> windows::core::Result<BSTR> {
    if hr.is_err() {
        let _ = ManuallyDrop::into_inner(out);
        return Err(hr.into());
    }
    Ok(ManuallyDrop::into_inner(out))
}

/// Finish a call whose sole `[out, retval]` is a COM interface pointer.
///
/// # Safety
///
/// `out` must be the raw out-pointer the vtable call wrote; on success it
/// is an AddRef'd `I*` whose ownership transfers to the returned wrapper.
#[inline]
unsafe fn iface_out<I: Interface>(
    hr: HRESULT,
    out: *mut c_void,
) -> windows::core::Result<I> {
    hr.ok()?;
    if out.is_null() {
        return Err(E_POINTER.into());
    }
    Ok(unsafe { I::from_raw(out) })
}

// --- IAccID ---------------------------------------------------------------
//
// : IUnknown. Vtable after IUnknown(3): get_accID (slot 1, used).
//
// The IDL declares get_accID as `long long*` on _WIN64 and `long*`
// otherwise. Both are `LONG_PTR` in the C++ backend (`getAccID`), so we
// bind the out-param as `*mut isize` (i32 on x86, i64 on x64) — no cfg.

windows_core::imp::define_interface!(
    IAccID,
    IAccID_Vtbl,
    0x81f9b44f_ba3a_4f5d_9b51_090c74a9b3a4
);
windows_core::imp::interface_hierarchy!(IAccID, IUnknown);

#[repr(C)]
pub struct IAccID_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub get_accID:
        unsafe extern "system" fn(this: *mut c_void, pID: *mut isize) -> HRESULT,
}

impl IAccID {
    /// The Acrobat accessible id (`LONG_PTR`), narrowed to `i32`. Acrobat
    /// only ever stores a 32-bit value here, so the narrowing is safe
    /// (mirrors the C++ `static_cast<long>` in `getAccID`).
    ///
    /// # Safety
    ///
    /// `self` must wrap a live `IAccID`.
    pub unsafe fn get_acc_id(&self) -> windows::core::Result<i32> {
        let mut id: isize = 0;
        let hr = (Interface::vtable(self).get_accID)(
            Interface::as_raw(self),
            &mut id,
        );
        hr.ok()?;
        Ok(id as i32)
    }
}

// --- IPDDomNode -----------------------------------------------------------
//
// : IDispatch. Vtable after IDispatch(7): GetParent, GetType, GetChildCount,
// GetChild, GetName, GetValue, IsSame, GetTextContent, GetLocation,
// GetFontInfo, GetFromID, GetIAccessible, ScrollTo, GetTextInLines.
// Used: GetType, GetChildCount, GetChild, GetName, GetValue, GetTextContent,
// GetFontInfo. All 14 slots are declared (placeholders for the unused
// ones) because IPDDomElement inherits this as its base vtable.

windows_core::imp::define_interface!(
    IPDDomNode,
    IPDDomNode_Vtbl,
    0x5007373a_20d7_458f_9ffb_abc900e3a831
);
impl core::ops::Deref for IPDDomNode {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IPDDomNode, IUnknown, IDispatch);

#[repr(C)]
pub struct IPDDomNode_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub GetParent: usize,
    pub GetType:
        unsafe extern "system" fn(this: *mut c_void, node_type: *mut i32) -> HRESULT,
    pub GetChildCount:
        unsafe extern "system" fn(this: *mut c_void, count: *mut i32) -> HRESULT,
    pub GetChild: unsafe extern "system" fn(
        this: *mut c_void,
        index: i32,
        child: *mut *mut c_void,
    ) -> HRESULT,
    pub GetName: unsafe extern "system" fn(
        this: *mut c_void,
        name: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub GetValue: unsafe extern "system" fn(
        this: *mut c_void,
        value: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub IsSame: usize,
    pub GetTextContent: unsafe extern "system" fn(
        this: *mut c_void,
        text: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub GetLocation: usize,
    pub GetFontInfo: unsafe extern "system" fn(
        this: *mut c_void,
        font_status: *mut i32,
        name: *mut ManuallyDrop<BSTR>,
        font_size: *mut f32,
        font_flags: *mut i32,
        red: *mut f32,
        green: *mut f32,
        blue: *mut f32,
    ) -> HRESULT,
    // Trailing IPDDomNode methods we don't call. They MUST still be
    // declared: IPDDomElement inherits this vtable as its base, so its own
    // methods sit after ALL of IPDDomNode's slots -- omitting these would
    // place IPDDomElement's methods 4 slots too early (wrong vtable offset
    // -> crash).
    pub GetFromID: usize,
    pub GetIAccessible: usize,
    pub ScrollTo: usize,
    pub GetTextInLines: usize,
}

/// Result of [`IPDDomNode::get_font_info`] (IDL `GetFontInfo`). `status`
/// is a `FontInfoState`; `flags` is a `PDDOM_FontStyle` bitmask.
pub struct PdFontInfo {
    pub status: i32,
    pub name: BSTR,
    pub size: f32,
    pub flags: i32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl IPDDomNode {
    /// `GetType` — the `CPDDomNodeType` of this node.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_type(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).GetType)(
            Interface::as_raw(self),
            &mut out,
        );
        hr.ok()?;
        Ok(out)
    }

    /// `GetChildCount`.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_child_count(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).GetChildCount)(
            Interface::as_raw(self),
            &mut out,
        );
        hr.ok()?;
        Ok(out)
    }

    /// `GetChild` — the child `IPDDomNode` at `index`.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_child(
        &self,
        index: i32,
    ) -> windows::core::Result<IPDDomNode> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).GetChild)(
            Interface::as_raw(self),
            index,
            &mut out,
        );
        iface_out(hr, out)
    }

    /// `GetName`.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_name(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetName)(
            Interface::as_raw(self),
            &mut out,
        );
        bstr_out(hr, out)
    }

    /// `GetValue`.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_value(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetValue)(
            Interface::as_raw(self),
            &mut out,
        );
        bstr_out(hr, out)
    }

    /// `GetTextContent` — the node's text (including descendants' text).
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_text_content(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetTextContent)(
            Interface::as_raw(self),
            &mut out,
        );
        bstr_out(hr, out)
    }

    /// `GetFontInfo`.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNode`.
    pub unsafe fn get_font_info(&self) -> windows::core::Result<PdFontInfo> {
        let mut status: i32 = 0;
        let mut name = ManuallyDrop::new(BSTR::default());
        let mut size: f32 = 0.0;
        let mut flags: i32 = 0;
        let mut red: f32 = 0.0;
        let mut green: f32 = 0.0;
        let mut blue: f32 = 0.0;
        let hr = (Interface::vtable(self).GetFontInfo)(
            Interface::as_raw(self),
            &mut status,
            &mut name,
            &mut size,
            &mut flags,
            &mut red,
            &mut green,
            &mut blue,
        );
        if hr.is_err() {
            let _ = ManuallyDrop::into_inner(name);
            return Err(hr.into());
        }
        Ok(PdFontInfo {
            status,
            name: ManuallyDrop::into_inner(name),
            size,
            flags,
            red,
            green,
            blue,
        })
    }
}

// --- IPDDomElement --------------------------------------------------------
//
// : IPDDomNode. Own vtable after IPDDomNode: GetTagName, GetStdName, GetID,
// GetAttribute. Used: GetStdName, GetID, GetAttribute.

windows_core::imp::define_interface!(
    IPDDomElement,
    IPDDomElement_Vtbl,
    0x198f17ae_b921_4308_9543_288d426a5c2b
);
impl core::ops::Deref for IPDDomElement {
    type Target = IPDDomNode;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(
    IPDDomElement,
    IUnknown,
    IDispatch,
    IPDDomNode
);

#[repr(C)]
pub struct IPDDomElement_Vtbl {
    pub base__: IPDDomNode_Vtbl,
    pub GetTagName: usize,
    pub GetStdName: unsafe extern "system" fn(
        this: *mut c_void,
        std_name: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub GetID: unsafe extern "system" fn(
        this: *mut c_void,
        id: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub GetAttribute: unsafe extern "system" fn(
        this: *mut c_void,
        attr: ManuallyDrop<BSTR>,
        owner: ManuallyDrop<BSTR>,
        attr_val: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
}

impl IPDDomElement {
    /// `GetStdName` — the standardised (Std) tag name for the element.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomElement`.
    pub unsafe fn get_std_name(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetStdName)(
            Interface::as_raw(self),
            &mut out,
        );
        bstr_out(hr, out)
    }

    /// `GetID` — the element's PDF `ID` attribute.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomElement`.
    pub unsafe fn get_id(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetID)(
            Interface::as_raw(self),
            &mut out,
        );
        bstr_out(hr, out)
    }

    /// `GetAttribute` — value of attribute `attr`. `owner` scopes the
    /// lookup (the backend passes a null `BSTR` for the default owner,
    /// matching the C++ `NULL`).
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomElement`; `attr`/`owner` must be
    /// valid `BSTR`s (or a null/empty `BSTR` for `owner`).
    pub unsafe fn get_attribute(
        &self,
        attr: &BSTR,
        owner: &BSTR,
    ) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).GetAttribute)(
            Interface::as_raw(self),
            in_param(attr),
            in_param(owner),
            &mut out,
        );
        bstr_out(hr, out)
    }
}

// --- IPDDomNodeExt --------------------------------------------------------
//
// : IUnknown. Vtable after IUnknown(3): GetState, Navigate, SetFocus,
// GetIndex, GetPageNum (slot 5, used — trailing slots omitted).

windows_core::imp::define_interface!(
    IPDDomNodeExt,
    IPDDomNodeExt_Vtbl,
    0x4a894040_247e_4aff_bb08_3489e9905235
);
windows_core::imp::interface_hierarchy!(IPDDomNodeExt, IUnknown);

#[repr(C)]
pub struct IPDDomNodeExt_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub GetState: usize,
    pub Navigate: usize,
    pub SetFocus: usize,
    pub GetIndex: usize,
    pub GetPageNum: unsafe extern "system" fn(
        this: *mut c_void,
        first_page: *mut i32,
        last_page: *mut i32,
    ) -> HRESULT,
}

impl IPDDomNodeExt {
    /// `GetPageNum` — the (0-based) first and last page this node spans.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomNodeExt`.
    pub unsafe fn get_page_num(&self) -> windows::core::Result<(i32, i32)> {
        let mut first: i32 = 0;
        let mut last: i32 = 0;
        let hr = (Interface::vtable(self).GetPageNum)(
            Interface::as_raw(self),
            &mut first,
            &mut last,
        );
        hr.ok()?;
        Ok((first, last))
    }
}

// --- IGetPDDomNode --------------------------------------------------------
//
// : IUnknown. Vtable after IUnknown(3): get_PDDomNode (slot 1, used).

windows_core::imp::define_interface!(
    IGetPDDomNode,
    IGetPDDomNode_Vtbl,
    0xf9f2fe81_f764_4bd0_afa5_5de841ddb625
);
windows_core::imp::interface_hierarchy!(IGetPDDomNode, IUnknown);

#[repr(C)]
pub struct IGetPDDomNode_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub get_PDDomNode: unsafe extern "system" fn(
        this: *mut c_void,
        var_id: ManuallyDrop<VARIANT>,
        node: *mut *mut c_void,
    ) -> HRESULT,
}

impl IGetPDDomNode {
    /// `get_PDDomNode` — the `IPDDomNode` for the accessible child `var_id`
    /// (the backend passes a `VT_I4` `CHILDID_SELF`).
    ///
    /// # Safety
    /// `self` must wrap a live `IGetPDDomNode`; `var_id` must be a valid
    /// `[in]` `VARIANT` that the caller keeps alive across the call.
    pub unsafe fn get_pddom_node(
        &self,
        var_id: &VARIANT,
    ) -> windows::core::Result<IPDDomNode> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_PDDomNode)(
            Interface::as_raw(self),
            in_param(var_id),
            &mut out,
        );
        iface_out(hr, out)
    }
}

// --- IPDDomDocPagination --------------------------------------------------
//
// : IUnknown. Vtable after IUnknown(3): GetInfo, LabelForPageNum (slot 2,
// used — GoToPage omitted as trailing).

windows_core::imp::define_interface!(
    IPDDomDocPagination,
    IPDDomDocPagination_Vtbl,
    0x8e6d1cb7_4dae_4de4_8ed9_15672a5f942f
);
windows_core::imp::interface_hierarchy!(IPDDomDocPagination, IUnknown);

#[repr(C)]
pub struct IPDDomDocPagination_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub GetInfo: usize,
    pub LabelForPageNum: unsafe extern "system" fn(
        this: *mut c_void,
        page_num: i32,
        page_label: *mut ManuallyDrop<BSTR>,
    ) -> HRESULT,
}

impl IPDDomDocPagination {
    /// `LabelForPageNum` — the textual page label for `page_num`, or the
    /// integer label if none exists.
    ///
    /// # Safety
    /// `self` must wrap a live `IPDDomDocPagination`.
    pub unsafe fn label_for_page_num(
        &self,
        page_num: i32,
    ) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).LabelForPageNum)(
            Interface::as_raw(self),
            page_num,
            &mut out,
        );
        bstr_out(hr, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    /// The IIDs `define_interface!` bakes in must match the IDL verbatim.
    #[test]
    fn iids_match_idl() {
        assert_eq!(
            IAccID::IID,
            GUID::from_u128(0x81f9b44f_ba3a_4f5d_9b51_090c74a9b3a4)
        );
        assert_eq!(
            IPDDomNode::IID,
            GUID::from_u128(0x5007373a_20d7_458f_9ffb_abc900e3a831)
        );
        assert_eq!(
            IPDDomNodeExt::IID,
            GUID::from_u128(0x4a894040_247e_4aff_bb08_3489e9905235)
        );
        assert_eq!(
            IPDDomElement::IID,
            GUID::from_u128(0x198f17ae_b921_4308_9543_288d426a5c2b)
        );
        assert_eq!(
            IGetPDDomNode::IID,
            GUID::from_u128(0xf9f2fe81_f764_4bd0_afa5_5de841ddb625)
        );
        assert_eq!(
            IPDDomDocPagination::IID,
            GUID::from_u128(0x8e6d1cb7_4dae_4de4_8ed9_15672a5f942f)
        );
    }

    /// Vtable layout guard: each `_Vtbl` must be its base vtable plus
    /// exactly the slots declared, so a miscounted placeholder shifts the
    /// size and trips this. `usize`-sized slots and fn-pointer slots are
    /// both one pointer wide.
    #[test]
    fn vtable_slot_counts() {
        let p = size_of::<usize>();
        // IAccID: IUnknown + 1
        assert_eq!(size_of::<IAccID_Vtbl>(), size_of::<IUnknown_Vtbl>() + p);
        // IPDDomNode: IDispatch + 14 (GetParent..GetTextInLines) -- the
        // full method set, since IPDDomElement inherits it as its base.
        assert_eq!(
            size_of::<IPDDomNode_Vtbl>(),
            size_of::<IDispatch_Vtbl>() + 14 * p
        );
        // IPDDomElement: IPDDomNode + 4 (GetTagName..GetAttribute)
        assert_eq!(
            size_of::<IPDDomElement_Vtbl>(),
            size_of::<IPDDomNode_Vtbl>() + 4 * p
        );
        // IPDDomNodeExt: IUnknown + 5 (GetState..GetPageNum)
        assert_eq!(
            size_of::<IPDDomNodeExt_Vtbl>(),
            size_of::<IUnknown_Vtbl>() + 5 * p
        );
        // IGetPDDomNode: IUnknown + 1
        assert_eq!(
            size_of::<IGetPDDomNode_Vtbl>(),
            size_of::<IUnknown_Vtbl>() + p
        );
        // IPDDomDocPagination: IUnknown + 2 (GetInfo, LabelForPageNum)
        assert_eq!(
            size_of::<IPDDomDocPagination_Vtbl>(),
            size_of::<IUnknown_Vtbl>() + 2 * p
        );
    }
}
