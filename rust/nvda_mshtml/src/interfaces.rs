//! Hand-rolled bindings for the MSHTML (Trident) COM interfaces the mshtml
//! vbuf backend consumes. windows-rs 0.58 ships no MSHTML module, so each
//! interface below is declared by hand with its vtable laid out to match the
//! SDK `MsHTML.h` `*Vtbl` structs exactly.
//!
//! Method order, IIDs, and signatures come verbatim from
//! `Windows Kits/10/Include/10.0.22621.0/um/MsHTML.h`. The set of methods
//! given real signatures (vs. `usize` placeholder slots) is exactly those the
//! C++ backend calls, surveyed from `nvdaHelper/vbufBackends/mshtml/{mshtml,node}.cpp`.
//! Slots BEFORE the deepest used method are placeholders (to keep offsets
//! correct); trailing slots after it are omitted. For interfaces used only as
//! a base (IHTMLDocument, IMarkupContainer) every own slot is a placeholder.
//!
//! A wrong vtable offset compiles but crashes at runtime, so the `tests` module
//! asserts both the IIDs and the exact slot counts against the SDK layout.
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::mem::ManuallyDrop;

use windows::core::{Interface, BSTR, HRESULT, IUnknown, IUnknown_Vtbl, VARIANT};
use windows::Win32::Foundation::{E_POINTER, VARIANT_BOOL};
use windows::Win32::System::Com::{IDispatch, IDispatch_Vtbl};

// --- Small helpers (copied from nvda_acrobat::interfaces) -----------------

/// Bitwise-copy an `[in]` COM value into a `ManuallyDrop` for a by-value
/// vtable parameter without transferring ownership. The original stays live
/// and is dropped by the caller; the callee treats `[in]` params as borrowed.
///
/// # Safety
/// The returned `ManuallyDrop<T>` aliases `*value` bit-for-bit; the caller must
/// keep `value` alive for the call and must not let both copies run `T`'s drop.
#[inline]
unsafe fn in_param<T>(value: &T) -> ManuallyDrop<T> {
    ManuallyDrop::new(unsafe { core::ptr::read(value) })
}

/// Finish a call whose sole `[out, retval]` is a server-allocated `BSTR`.
#[inline]
unsafe fn bstr_out(hr: HRESULT, out: ManuallyDrop<BSTR>) -> windows::core::Result<BSTR> {
    if hr.is_err() {
        let _ = ManuallyDrop::into_inner(out);
        return Err(hr.into());
    }
    Ok(ManuallyDrop::into_inner(out))
}

/// Finish a call whose sole `[out, retval]` is a server-owned `VARIANT`. On
/// success the returned `VARIANT` `VariantClear`s on drop; on failure the
/// partially-written value is reclaimed so it is cleared.
#[inline]
unsafe fn variant_out(hr: HRESULT, out: ManuallyDrop<VARIANT>) -> windows::core::Result<VARIANT> {
    if hr.is_err() {
        let _ = ManuallyDrop::into_inner(out);
        return Err(hr.into());
    }
    Ok(ManuallyDrop::into_inner(out))
}

/// Finish a call whose sole `[out, retval]` is a COM interface pointer.
///
/// # Safety
/// `out` must be the raw out-pointer the vtable call wrote; on success it is an
/// AddRef'd `I*` whose ownership transfers to the returned wrapper.
#[inline]
unsafe fn iface_out<I: Interface>(hr: HRESULT, out: *mut c_void) -> windows::core::Result<I> {
    hr.ok()?;
    if out.is_null() {
        return Err(E_POINTER.into());
    }
    Ok(unsafe { I::from_raw(out) })
}

// --- IHTMLDOMNode ----------------------------------------------------------
//
// Trident DOM node. Used: get_parentNode, get_childNodes, get_attributes, get_nodeName.
// Bound method vtbl slots: get_parentNode=vtbl slot 8, get_childNodes=vtbl slot 10, get_attributes=vtbl slot 11, get_nodeName=vtbl slot 20.
windows_core::imp::define_interface!(
    IHTMLDOMNode,
    IHTMLDOMNode_Vtbl,
    0x3050f5da_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDOMNode {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDOMNode, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDOMNode_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_nodeType: usize,
    pub get_parentNode: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
    pub hasChildNodes: usize,
    pub get_childNodes: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
    pub get_attributes: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
    pub insertBefore: usize,
    pub removeChild: usize,
    pub replaceChild: usize,
    pub cloneNode: usize,
    pub removeNode: usize,
    pub swapNode: usize,
    pub replaceNode: usize,
    pub appendChild: usize,
    pub get_nodeName: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
}

impl IHTMLDOMNode {
    /// `get_parentNode` — the parent DOM node.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_parent_node(&self) -> windows::core::Result<IHTMLDOMNode> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_parentNode)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }

    /// `get_childNodes` — child collection (QI to IHTMLDOMChildrenCollection).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_child_nodes(&self) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_childNodes)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }

    /// `get_attributes` — attribute collection (QI to IHTMLAttributeCollection2).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_attributes(&self) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_attributes)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }

    /// `get_nodeName` — the tag/node name (e.g. `DIV`, `#text`).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_node_name(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_nodeName)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }
}

// --- IHTMLDOMNode2 ---------------------------------------------------------
//
// Used: get_ownerDocument (owning document as IDispatch).
// Bound method vtbl slots: get_ownerDocument=vtbl slot 7.
windows_core::imp::define_interface!(
    IHTMLDOMNode2,
    IHTMLDOMNode2_Vtbl,
    0x3050f80b_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDOMNode2 {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDOMNode2, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDOMNode2_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_ownerDocument: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
}

impl IHTMLDOMNode2 {
    /// `get_ownerDocument` — owning document (QI to IMarkupContainer2).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_owner_document(&self) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_ownerDocument)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }
}

// --- IHTMLDOMAttribute -----------------------------------------------------
//
// Attribute node returned by IHTMLAttributeCollection2::getNamedItem. Used: get_nodeValue (a VARIANT).
// Bound method vtbl slots: get_nodeValue=vtbl slot 9.
windows_core::imp::define_interface!(
    IHTMLDOMAttribute,
    IHTMLDOMAttribute_Vtbl,
    0x3050f4b0_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDOMAttribute {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDOMAttribute, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDOMAttribute_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_nodeName: usize,
    pub put_nodeValue: usize,
    pub get_nodeValue: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<VARIANT>) -> HRESULT,
}

impl IHTMLDOMAttribute {
    /// `get_nodeValue` — the attribute value as a VARIANT.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_node_value(&self) -> windows::core::Result<VARIANT> {
        let mut out = ManuallyDrop::new(VARIANT::default());
        let hr = (Interface::vtable(self).get_nodeValue)(Interface::as_raw(self), &mut out);
        variant_out(hr, out)
    }
}

// --- IHTMLDOMTextNode ------------------------------------------------------
//
// Text node. Used: get_data.
// Bound method vtbl slots: get_data=vtbl slot 8.
windows_core::imp::define_interface!(
    IHTMLDOMTextNode,
    IHTMLDOMTextNode_Vtbl,
    0x3050f4b1_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDOMTextNode {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDOMTextNode, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDOMTextNode_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub put_data: usize,
    pub get_data: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
}

impl IHTMLDOMTextNode {
    /// `get_data` — the text content of this text node.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_data(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_data)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }
}

// --- IHTMLElementCollection ------------------------------------------------
//
// Element collection from IHTMLElement2::getElementsByTagName. Used: get_length, item (two VARIANT args).
// Bound method vtbl slots: get_length=vtbl slot 9, item=vtbl slot 11.
windows_core::imp::define_interface!(
    IHTMLElementCollection,
    IHTMLElementCollection_Vtbl,
    0x3050f21f_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLElementCollection {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLElementCollection, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLElementCollection_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub toString: usize,
    pub put_length: usize,
    pub get_length: unsafe extern "system" fn(this: *mut c_void, p: *mut i32) -> HRESULT,
    pub get__newEnum: usize,
    pub item: unsafe extern "system" fn(this: *mut c_void, name: ManuallyDrop<VARIANT>, index: ManuallyDrop<VARIANT>, pdisp: *mut *mut c_void) -> HRESULT,
}

impl IHTMLElementCollection {
    /// `get_length` — number of elements.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_length(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_length)(Interface::as_raw(self), &mut out);
        hr.ok()?;
        Ok(out)
    }

    /// `item` — the element at `index`. Both selectors are `[in]` VARIANTs
    /// (the backend passes `VT_I4` values).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `name`/`index` must be valid
    /// VARIANTs kept alive across the call.
    pub unsafe fn item(&self, name: &VARIANT, index: &VARIANT) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).item)(
            Interface::as_raw(self),
            in_param(name),
            in_param(index),
            &mut out,
        );
        iface_out(hr, out)
    }
}

// --- IHTMLAttributeCollection2 ---------------------------------------------
//
// Attribute collection. Used: getNamedItem.
// Bound method vtbl slots: getNamedItem=vtbl slot 7.
windows_core::imp::define_interface!(
    IHTMLAttributeCollection2,
    IHTMLAttributeCollection2_Vtbl,
    0x3050f80a_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLAttributeCollection2 {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLAttributeCollection2, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLAttributeCollection2_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub getNamedItem: unsafe extern "system" fn(this: *mut c_void, bstrName: ManuallyDrop<BSTR>, newretNode: *mut *mut c_void) -> HRESULT,
}

impl IHTMLAttributeCollection2 {
    /// `getNamedItem` — the attribute node named `name`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `name` must be a valid BSTR
    /// kept alive across the call.
    pub unsafe fn get_named_item(&self, name: &BSTR) -> windows::core::Result<IHTMLDOMAttribute> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).getNamedItem)(
            Interface::as_raw(self),
            in_param(name),
            &mut out,
        );
        iface_out(hr, out)
    }
}

// --- IHTMLDOMChildrenCollection --------------------------------------------
//
// Child node collection. Used: get_length, item (long index).
// Bound method vtbl slots: get_length=vtbl slot 7, item=vtbl slot 9.
windows_core::imp::define_interface!(
    IHTMLDOMChildrenCollection,
    IHTMLDOMChildrenCollection_Vtbl,
    0x3050f5ab_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDOMChildrenCollection {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDOMChildrenCollection, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDOMChildrenCollection_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_length: unsafe extern "system" fn(this: *mut c_void, p: *mut i32) -> HRESULT,
    pub get__newEnum: usize,
    pub item: unsafe extern "system" fn(this: *mut c_void, index: i32, ppItem: *mut *mut c_void) -> HRESULT,
}

impl IHTMLDOMChildrenCollection {
    /// `get_length` — number of child nodes.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_length(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_length)(Interface::as_raw(self), &mut out);
        hr.ok()?;
        Ok(out)
    }

    /// `item` — the child node (as IDispatch) at `index`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn item(&self, index: i32) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).item)(Interface::as_raw(self), index, &mut out);
        iface_out(hr, out)
    }
}

// --- IHTMLUniqueName -------------------------------------------------------
//
// Used: get_uniqueNumber (the stable per-element id NVDA keys nodes on).
// Bound method vtbl slots: get_uniqueNumber=vtbl slot 7.
windows_core::imp::define_interface!(
    IHTMLUniqueName,
    IHTMLUniqueName_Vtbl,
    0x3050f4d0_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLUniqueName {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLUniqueName, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLUniqueName_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_uniqueNumber: unsafe extern "system" fn(this: *mut c_void, p: *mut i32) -> HRESULT,
}

impl IHTMLUniqueName {
    /// `get_uniqueNumber` — the element's stable unique number.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_unique_number(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_uniqueNumber)(Interface::as_raw(self), &mut out);
        hr.ok()?;
        Ok(out)
    }
}

// --- IHTMLElement ----------------------------------------------------------
//
// Core element. Used: getAttribute, get_tagName, get_parentElement, get_document.
// Bound method vtbl slots: getAttribute=vtbl slot 8, get_tagName=vtbl slot 14, get_parentElement=vtbl slot 15, get_document=vtbl slot 39.
windows_core::imp::define_interface!(
    IHTMLElement,
    IHTMLElement_Vtbl,
    0x3050f1ff_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLElement {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLElement, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLElement_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub setAttribute: usize,
    pub getAttribute: unsafe extern "system" fn(this: *mut c_void, strAttributeName: ManuallyDrop<BSTR>, lFlags: i32, AttributeValue: *mut ManuallyDrop<VARIANT>) -> HRESULT,
    pub removeAttribute: usize,
    pub put_className: usize,
    pub get_className: usize,
    pub put_id: usize,
    pub get_id: usize,
    pub get_tagName: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_parentElement: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
    pub get_style: usize,
    pub put_onhelp: usize,
    pub get_onhelp: usize,
    pub put_onclick: usize,
    pub get_onclick: usize,
    pub put_ondblclick: usize,
    pub get_ondblclick: usize,
    pub put_onkeydown: usize,
    pub get_onkeydown: usize,
    pub put_onkeyup: usize,
    pub get_onkeyup: usize,
    pub put_onkeypress: usize,
    pub get_onkeypress: usize,
    pub put_onmouseout: usize,
    pub get_onmouseout: usize,
    pub put_onmouseover: usize,
    pub get_onmouseover: usize,
    pub put_onmousemove: usize,
    pub get_onmousemove: usize,
    pub put_onmousedown: usize,
    pub get_onmousedown: usize,
    pub put_onmouseup: usize,
    pub get_onmouseup: usize,
    pub get_document: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
}

impl IHTMLElement {
    /// `getAttribute` — value of attribute `name`. `flags` matches the IDL
    /// `lFlags` (the backend passes `2`, i.e. case-sensitive exact match).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `name` must be a valid BSTR
    /// kept alive across the call.
    pub unsafe fn get_attribute(&self, name: &BSTR, flags: i32) -> windows::core::Result<VARIANT> {
        let mut out = ManuallyDrop::new(VARIANT::default());
        let hr = (Interface::vtable(self).getAttribute)(
            Interface::as_raw(self),
            in_param(name),
            flags,
            &mut out,
        );
        variant_out(hr, out)
    }

    /// `get_tagName` — the element tag name.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_tag_name(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_tagName)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_parentElement` — the parent element.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_parent_element(&self) -> windows::core::Result<IHTMLElement> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_parentElement)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }

    /// `get_document` — the element's document (QI to IHTMLDocument3).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_document(&self) -> windows::core::Result<IDispatch> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_document)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }
}

// --- IHTMLCurrentStyle -----------------------------------------------------
//
// Computed style. Used (BSTR): fontFamily, fontStyle, textAlign, textDecoration, display, visibility, listStyleType. Used (VARIANT): fontWeight, fontSize, verticalAlign.
// Bound method vtbl slots: get_fontFamily=vtbl slot 11, get_fontStyle=vtbl slot 12, get_fontWeight=vtbl slot 14, get_fontSize=vtbl slot 15, get_textAlign=vtbl slot 40, get_textDecoration=vtbl slot 41, get_display=vtbl slot 42, get_visibility=vtbl slot 43, get_verticalAlign=vtbl slot 48, get_listStyleType=vtbl slot 55.
windows_core::imp::define_interface!(
    IHTMLCurrentStyle,
    IHTMLCurrentStyle_Vtbl,
    0x3050f3db_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLCurrentStyle {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLCurrentStyle, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLCurrentStyle_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_position: usize,
    pub get_styleFloat: usize,
    pub get_color: usize,
    pub get_backgroundColor: usize,
    pub get_fontFamily: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_fontStyle: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_fontVariant: usize,
    pub get_fontWeight: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<VARIANT>) -> HRESULT,
    pub get_fontSize: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<VARIANT>) -> HRESULT,
    pub get_backgroundImage: usize,
    pub get_backgroundPositionX: usize,
    pub get_backgroundPositionY: usize,
    pub get_backgroundRepeat: usize,
    pub get_borderLeftColor: usize,
    pub get_borderTopColor: usize,
    pub get_borderRightColor: usize,
    pub get_borderBottomColor: usize,
    pub get_borderTopStyle: usize,
    pub get_borderRightStyle: usize,
    pub get_borderBottomStyle: usize,
    pub get_borderLeftStyle: usize,
    pub get_borderTopWidth: usize,
    pub get_borderRightWidth: usize,
    pub get_borderBottomWidth: usize,
    pub get_borderLeftWidth: usize,
    pub get_left: usize,
    pub get_top: usize,
    pub get_width: usize,
    pub get_height: usize,
    pub get_paddingLeft: usize,
    pub get_paddingTop: usize,
    pub get_paddingRight: usize,
    pub get_paddingBottom: usize,
    pub get_textAlign: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_textDecoration: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_display: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_visibility: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
    pub get_zIndex: usize,
    pub get_letterSpacing: usize,
    pub get_lineHeight: usize,
    pub get_textIndent: usize,
    pub get_verticalAlign: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<VARIANT>) -> HRESULT,
    pub get_backgroundAttachment: usize,
    pub get_marginTop: usize,
    pub get_marginRight: usize,
    pub get_marginBottom: usize,
    pub get_marginLeft: usize,
    pub get_clear: usize,
    pub get_listStyleType: unsafe extern "system" fn(this: *mut c_void, p: *mut ManuallyDrop<BSTR>) -> HRESULT,
}

impl IHTMLCurrentStyle {
    /// `get_fontFamily`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_font_family(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_fontFamily)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_fontStyle`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_font_style(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_fontStyle)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_textAlign`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_text_align(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_textAlign)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_textDecoration`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_text_decoration(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_textDecoration)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_display`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_display(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_display)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_visibility`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_visibility(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_visibility)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_listStyleType`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_list_style_type(&self) -> windows::core::Result<BSTR> {
        let mut out = ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_listStyleType)(Interface::as_raw(self), &mut out);
        bstr_out(hr, out)
    }

    /// `get_fontWeight` (VARIANT; the backend reads VT_I4).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_font_weight(&self) -> windows::core::Result<VARIANT> {
        let mut out = ManuallyDrop::new(VARIANT::default());
        let hr = (Interface::vtable(self).get_fontWeight)(Interface::as_raw(self), &mut out);
        variant_out(hr, out)
    }

    /// `get_fontSize` (VARIANT).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_font_size(&self) -> windows::core::Result<VARIANT> {
        let mut out = ManuallyDrop::new(VARIANT::default());
        let hr = (Interface::vtable(self).get_fontSize)(Interface::as_raw(self), &mut out);
        variant_out(hr, out)
    }

    /// `get_verticalAlign` (VARIANT).
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_vertical_align(&self) -> windows::core::Result<VARIANT> {
        let mut out = ManuallyDrop::new(VARIANT::default());
        let hr = (Interface::vtable(self).get_verticalAlign)(Interface::as_raw(self), &mut out);
        variant_out(hr, out)
    }
}

// --- IHTMLElement2 ---------------------------------------------------------
//
// Used: get_currentStyle, getElementsByTagName. getElementsByTagName sits deep (slot 7+97), so 98 slots are declared.
// Bound method vtbl slots: get_currentStyle=vtbl slot 40, getElementsByTagName=vtbl slot 104.
windows_core::imp::define_interface!(
    IHTMLElement2,
    IHTMLElement2_Vtbl,
    0x3050f434_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLElement2 {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLElement2, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLElement2_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_scopeName: usize,
    pub setCapture: usize,
    pub releaseCapture: usize,
    pub put_onlosecapture: usize,
    pub get_onlosecapture: usize,
    pub componentFromPoint: usize,
    pub doScroll: usize,
    pub put_onscroll: usize,
    pub get_onscroll: usize,
    pub put_ondrag: usize,
    pub get_ondrag: usize,
    pub put_ondragend: usize,
    pub get_ondragend: usize,
    pub put_ondragenter: usize,
    pub get_ondragenter: usize,
    pub put_ondragover: usize,
    pub get_ondragover: usize,
    pub put_ondragleave: usize,
    pub get_ondragleave: usize,
    pub put_ondrop: usize,
    pub get_ondrop: usize,
    pub put_onbeforecut: usize,
    pub get_onbeforecut: usize,
    pub put_oncut: usize,
    pub get_oncut: usize,
    pub put_onbeforecopy: usize,
    pub get_onbeforecopy: usize,
    pub put_oncopy: usize,
    pub get_oncopy: usize,
    pub put_onbeforepaste: usize,
    pub get_onbeforepaste: usize,
    pub put_onpaste: usize,
    pub get_onpaste: usize,
    pub get_currentStyle: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
    pub put_onpropertychange: usize,
    pub get_onpropertychange: usize,
    pub getClientRects: usize,
    pub getBoundingClientRect: usize,
    pub setExpression: usize,
    pub getExpression: usize,
    pub removeExpression: usize,
    pub put_tabIndex: usize,
    pub get_tabIndex: usize,
    pub focus: usize,
    pub put_accessKey: usize,
    pub get_accessKey: usize,
    pub put_onblur: usize,
    pub get_onblur: usize,
    pub put_onfocus: usize,
    pub get_onfocus: usize,
    pub put_onresize: usize,
    pub get_onresize: usize,
    pub blur: usize,
    pub addFilter: usize,
    pub removeFilter: usize,
    pub get_clientHeight: usize,
    pub get_clientWidth: usize,
    pub get_clientTop: usize,
    pub get_clientLeft: usize,
    pub attachEvent: usize,
    pub detachEvent: usize,
    pub get_readyState: usize,
    pub put_onreadystatechange: usize,
    pub get_onreadystatechange: usize,
    pub put_onrowsdelete: usize,
    pub get_onrowsdelete: usize,
    pub put_onrowsinserted: usize,
    pub get_onrowsinserted: usize,
    pub put_oncellchange: usize,
    pub get_oncellchange: usize,
    pub put_dir: usize,
    pub get_dir: usize,
    pub createControlRange: usize,
    pub get_scrollHeight: usize,
    pub get_scrollWidth: usize,
    pub put_scrollTop: usize,
    pub get_scrollTop: usize,
    pub put_scrollLeft: usize,
    pub get_scrollLeft: usize,
    pub clearAttributes: usize,
    pub mergeAttributes: usize,
    pub put_oncontextmenu: usize,
    pub get_oncontextmenu: usize,
    pub insertAdjacentElement: usize,
    pub applyElement: usize,
    pub getAdjacentText: usize,
    pub replaceAdjacentText: usize,
    pub get_canHaveChildren: usize,
    pub addBehavior: usize,
    pub removeBehavior: usize,
    pub get_runtimeStyle: usize,
    pub get_behaviorUrns: usize,
    pub put_tagUrn: usize,
    pub get_tagUrn: usize,
    pub put_onbeforeeditfocus: usize,
    pub get_onbeforeeditfocus: usize,
    pub get_readyStateValue: usize,
    pub getElementsByTagName: unsafe extern "system" fn(this: *mut c_void, v: ManuallyDrop<BSTR>, pelColl: *mut *mut c_void) -> HRESULT,
}

impl IHTMLElement2 {
    /// `get_currentStyle` — the computed style object.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_current_style(&self) -> windows::core::Result<IHTMLCurrentStyle> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_currentStyle)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }

    /// `getElementsByTagName` — descendant elements with tag `name`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `name` must be a valid BSTR
    /// kept alive across the call.
    pub unsafe fn get_elements_by_tag_name(&self, name: &BSTR) -> windows::core::Result<IHTMLElementCollection> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).getElementsByTagName)(
            Interface::as_raw(self),
            in_param(name),
            &mut out,
        );
        iface_out(hr, out)
    }
}

// --- IHTMLElement3 ---------------------------------------------------------
//
// Used: get_isContentEditable.
// Bound method vtbl slots: get_isContentEditable=vtbl slot 21.
windows_core::imp::define_interface!(
    IHTMLElement3,
    IHTMLElement3_Vtbl,
    0x3050f673_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLElement3 {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLElement3, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLElement3_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub mergeAttributes: usize,
    pub get_isMultiLine: usize,
    pub get_canHaveHTML: usize,
    pub put_onlayoutcomplete: usize,
    pub get_onlayoutcomplete: usize,
    pub put_onpage: usize,
    pub get_onpage: usize,
    pub put_inflateBlock: usize,
    pub get_inflateBlock: usize,
    pub put_onbeforedeactivate: usize,
    pub get_onbeforedeactivate: usize,
    pub setActive: usize,
    pub put_contentEditable: usize,
    pub get_contentEditable: usize,
    pub get_isContentEditable: unsafe extern "system" fn(this: *mut c_void, p: *mut VARIANT_BOOL) -> HRESULT,
}

impl IHTMLElement3 {
    /// `get_isContentEditable` — whether the element is content-editable.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_is_content_editable(&self) -> windows::core::Result<VARIANT_BOOL> {
        let mut out = VARIANT_BOOL(0);
        let hr = (Interface::vtable(self).get_isContentEditable)(Interface::as_raw(self), &mut out);
        hr.ok()?;
        Ok(out)
    }
}

// --- IHTMLDocument ---------------------------------------------------------
//
// Base interface of IHTMLDocument2. No method is called directly; it exists only so IHTMLDocument2's base vtable is complete (its single own slot get_Script is a placeholder).
// Bound method vtbl slots: .
windows_core::imp::define_interface!(
    IHTMLDocument,
    IHTMLDocument_Vtbl,
    0x626fc520_a41e_11cf_a731_00a0c9082637
);
impl core::ops::Deref for IHTMLDocument {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDocument, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDocument_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub get_Script: usize,
}

// --- IHTMLDocument2 --------------------------------------------------------
//
// Derives IHTMLDocument (NOT IDispatch directly), so base__ is a full IHTMLDocument_Vtbl. Used: get_body.
// Bound method vtbl slots: get_body=vtbl slot 9.
windows_core::imp::define_interface!(
    IHTMLDocument2,
    IHTMLDocument2_Vtbl,
    0x332c4425_26cb_11d0_b483_00c04fd90119
);
impl core::ops::Deref for IHTMLDocument2 {
    type Target = IHTMLDocument;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDocument2, IUnknown, IDispatch, IHTMLDocument);

#[repr(C)]
pub struct IHTMLDocument2_Vtbl {
    pub base__: IHTMLDocument_Vtbl,
    pub get_all: usize,
    pub get_body: unsafe extern "system" fn(this: *mut c_void, p: *mut *mut c_void) -> HRESULT,
}

impl IHTMLDocument2 {
    /// `get_body` — the document body element.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn get_body(&self) -> windows::core::Result<IHTMLElement> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_body)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }
}

// --- IHTMLDocument3 --------------------------------------------------------
//
// Used: getElementById.
// Bound method vtbl slots: getElementById=vtbl slot 46.
windows_core::imp::define_interface!(
    IHTMLDocument3,
    IHTMLDocument3_Vtbl,
    0x3050f485_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IHTMLDocument3 {
    type Target = IDispatch;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IHTMLDocument3, IUnknown, IDispatch);

#[repr(C)]
pub struct IHTMLDocument3_Vtbl {
    pub base__: IDispatch_Vtbl,
    pub releaseCapture: usize,
    pub recalc: usize,
    pub createTextNode: usize,
    pub get_documentElement: usize,
    pub get_uniqueID: usize,
    pub attachEvent: usize,
    pub detachEvent: usize,
    pub put_onrowsdelete: usize,
    pub get_onrowsdelete: usize,
    pub put_onrowsinserted: usize,
    pub get_onrowsinserted: usize,
    pub put_oncellchange: usize,
    pub get_oncellchange: usize,
    pub put_ondatasetchanged: usize,
    pub get_ondatasetchanged: usize,
    pub put_ondataavailable: usize,
    pub get_ondataavailable: usize,
    pub put_ondatasetcomplete: usize,
    pub get_ondatasetcomplete: usize,
    pub put_onpropertychange: usize,
    pub get_onpropertychange: usize,
    pub put_dir: usize,
    pub get_dir: usize,
    pub put_oncontextmenu: usize,
    pub get_oncontextmenu: usize,
    pub put_onstop: usize,
    pub get_onstop: usize,
    pub createDocumentFragment: usize,
    pub get_parentDocument: usize,
    pub put_enableDownload: usize,
    pub get_enableDownload: usize,
    pub put_baseUrl: usize,
    pub get_baseUrl: usize,
    pub get_childNodes: usize,
    pub put_inheritStyleSheets: usize,
    pub get_inheritStyleSheets: usize,
    pub put_onbeforeeditfocus: usize,
    pub get_onbeforeeditfocus: usize,
    pub getElementsByName: usize,
    pub getElementById: unsafe extern "system" fn(this: *mut c_void, v: ManuallyDrop<BSTR>, pel: *mut *mut c_void) -> HRESULT,
}

impl IHTMLDocument3 {
    /// `getElementById` — the element whose id is `id`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `id` must be a valid BSTR
    /// kept alive across the call.
    pub unsafe fn get_element_by_id(&self, id: &BSTR) -> windows::core::Result<IHTMLElement> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).getElementById)(
            Interface::as_raw(self),
            in_param(id),
            &mut out,
        );
        iface_out(hr, out)
    }
}

// --- IMarkupPointer --------------------------------------------------------
//
// IUnknown-derived. Used: CurrentScope (returns the innermost IHTMLElement).
// Bound method vtbl slots: CurrentScope=vtbl slot 16.
windows_core::imp::define_interface!(
    IMarkupPointer,
    IMarkupPointer_Vtbl,
    0x3050f49f_98b5_11cf_bb82_00aa00bdce0b
);
windows_core::imp::interface_hierarchy!(IMarkupPointer, IUnknown);

#[repr(C)]
pub struct IMarkupPointer_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub OwningDoc: usize,
    pub Gravity: usize,
    pub SetGravity: usize,
    pub Cling: usize,
    pub SetCling: usize,
    pub Unposition: usize,
    pub IsPositioned: usize,
    pub GetContainer: usize,
    pub MoveAdjacentToElement: usize,
    pub MoveToPointer: usize,
    pub MoveToContainer: usize,
    pub Left: usize,
    pub Right: usize,
    pub CurrentScope: unsafe extern "system" fn(this: *mut c_void, ppElemCurrent: *mut *mut c_void) -> HRESULT,
}

impl IMarkupPointer {
    /// `CurrentScope` — the innermost element at this pointer.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn current_scope(&self) -> windows::core::Result<IHTMLElement> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).CurrentScope)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }
}

// --- IMarkupContainer ------------------------------------------------------
//
// IUnknown-derived base of IMarkupContainer2. No method called directly; its single own slot OwningDoc is a placeholder to complete IMarkupContainer2's base vtable.
// Bound method vtbl slots: .
windows_core::imp::define_interface!(
    IMarkupContainer,
    IMarkupContainer_Vtbl,
    0x3050f5f9_98b5_11cf_bb82_00aa00bdce0b
);
windows_core::imp::interface_hierarchy!(IMarkupContainer, IUnknown);

#[repr(C)]
pub struct IMarkupContainer_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub OwningDoc: usize,
}

// --- IMarkupContainer2 -----------------------------------------------------
//
// Derives IMarkupContainer. Used: RegisterForDirtyRange, UnRegisterForDirtyRange, GetAndClearDirtyRange (CreateChangeLog before them is a placeholder).
// Bound method vtbl slots: RegisterForDirtyRange=vtbl slot 5, UnRegisterForDirtyRange=vtbl slot 6, GetAndClearDirtyRange=vtbl slot 7.
windows_core::imp::define_interface!(
    IMarkupContainer2,
    IMarkupContainer2_Vtbl,
    0x3050f648_98b5_11cf_bb82_00aa00bdce0b
);
impl core::ops::Deref for IMarkupContainer2 {
    type Target = IMarkupContainer;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IMarkupContainer2, IUnknown, IMarkupContainer);

#[repr(C)]
pub struct IMarkupContainer2_Vtbl {
    pub base__: IMarkupContainer_Vtbl,
    pub CreateChangeLog: usize,
    pub RegisterForDirtyRange: unsafe extern "system" fn(this: *mut c_void, pChangeSink: *mut c_void, pdwCookie: *mut u32) -> HRESULT,
    pub UnRegisterForDirtyRange: unsafe extern "system" fn(this: *mut c_void, dwCookie: u32) -> HRESULT,
    pub GetAndClearDirtyRange: unsafe extern "system" fn(this: *mut c_void, dwCookie: u32, pIPointerBegin: *mut c_void, pIPointerEnd: *mut c_void) -> HRESULT,
}

impl IMarkupContainer2 {
    /// `RegisterForDirtyRange` — subscribe `sink` (an `IHTMLChangeSink*`) to
    /// dirty-range notifications; returns the registration cookie.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer; `sink` must be a live
    /// `IHTMLChangeSink` COM pointer that stays alive until unregistered.
    pub unsafe fn register_for_dirty_range(&self, sink: *mut c_void) -> windows::core::Result<u32> {
        let mut cookie: u32 = 0;
        let hr = (Interface::vtable(self).RegisterForDirtyRange)(
            Interface::as_raw(self),
            sink,
            &mut cookie,
        );
        hr.ok()?;
        Ok(cookie)
    }

    /// `UnRegisterForDirtyRange` — cancel the registration for `cookie`.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn unregister_for_dirty_range(&self, cookie: u32) -> windows::core::Result<()> {
        let hr = (Interface::vtable(self).UnRegisterForDirtyRange)(Interface::as_raw(self), cookie);
        hr.ok()
    }

    /// `GetAndClearDirtyRange` — move `begin`/`end` markup pointers to bound
    /// the dirty range for `cookie` and clear it.
    ///
    /// # Safety
    /// `self`, `begin`, `end` must wrap live interface pointers.
    pub unsafe fn get_and_clear_dirty_range(
        &self,
        cookie: u32,
        begin: &IMarkupPointer,
        end: &IMarkupPointer,
    ) -> windows::core::Result<()> {
        let hr = (Interface::vtable(self).GetAndClearDirtyRange)(
            Interface::as_raw(self),
            cookie,
            Interface::as_raw(begin),
            Interface::as_raw(end),
        );
        hr.ok()
    }
}

// --- IMarkupServices2 ------------------------------------------------------
//
// Derives IMarkupServices : IUnknown. The only method used is IMarkupServices::CreateMarkupPointer, which is the FIRST slot after IUnknown, so we model the base as IUnknown_Vtbl and declare just that one slot.
// Bound method vtbl slots: CreateMarkupPointer=vtbl slot 3.
windows_core::imp::define_interface!(
    IMarkupServices2,
    IMarkupServices2_Vtbl,
    0x3050f682_98b5_11cf_bb82_00aa00bdce0b
);
windows_core::imp::interface_hierarchy!(IMarkupServices2, IUnknown);

#[repr(C)]
pub struct IMarkupServices2_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub CreateMarkupPointer: unsafe extern "system" fn(this: *mut c_void, ppPointer: *mut *mut c_void) -> HRESULT,
}

impl IMarkupServices2 {
    /// `CreateMarkupPointer` — allocate a fresh markup pointer.
    ///
    /// # Safety
    /// `self` must wrap a live interface pointer.
    pub unsafe fn create_markup_pointer(&self) -> windows::core::Result<IMarkupPointer> {
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).CreateMarkupPointer)(Interface::as_raw(self), &mut out);
        iface_out(hr, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;
    use windows::core::GUID;

    /// The IIDs baked in by `define_interface!` must match `MsHTML.h` verbatim.
    #[test]
    fn iids_match_idl() {
        assert_eq!(IHTMLDOMNode::IID, GUID::from_u128(0x3050f5da_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLDOMNode2::IID, GUID::from_u128(0x3050f80b_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLDOMAttribute::IID, GUID::from_u128(0x3050f4b0_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLDOMTextNode::IID, GUID::from_u128(0x3050f4b1_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLElementCollection::IID, GUID::from_u128(0x3050f21f_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLAttributeCollection2::IID, GUID::from_u128(0x3050f80a_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLDOMChildrenCollection::IID, GUID::from_u128(0x3050f5ab_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLUniqueName::IID, GUID::from_u128(0x3050f4d0_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLElement::IID, GUID::from_u128(0x3050f1ff_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLCurrentStyle::IID, GUID::from_u128(0x3050f3db_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLElement2::IID, GUID::from_u128(0x3050f434_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLElement3::IID, GUID::from_u128(0x3050f673_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IHTMLDocument::IID, GUID::from_u128(0x626fc520_a41e_11cf_a731_00a0c9082637));
        assert_eq!(IHTMLDocument2::IID, GUID::from_u128(0x332c4425_26cb_11d0_b483_00c04fd90119));
        assert_eq!(IHTMLDocument3::IID, GUID::from_u128(0x3050f485_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IMarkupPointer::IID, GUID::from_u128(0x3050f49f_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IMarkupContainer::IID, GUID::from_u128(0x3050f5f9_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IMarkupContainer2::IID, GUID::from_u128(0x3050f648_98b5_11cf_bb82_00aa00bdce0b));
        assert_eq!(IMarkupServices2::IID, GUID::from_u128(0x3050f682_98b5_11cf_bb82_00aa00bdce0b));
    }

    /// Each `_Vtbl` must be its base vtable plus exactly the declared slots, so
    /// a miscounted placeholder shifts the size and trips this. Placeholder
    /// (`usize`) and fn-pointer slots are both one pointer wide.
    #[test]
    fn vtable_slot_counts() {
        let p = size_of::<usize>();
        assert_eq!(size_of::<IHTMLDOMNode_Vtbl>(), size_of::<IDispatch_Vtbl>() + 14 * p);
        assert_eq!(size_of::<IHTMLDOMNode2_Vtbl>(), size_of::<IDispatch_Vtbl>() + 1 * p);
        assert_eq!(size_of::<IHTMLDOMAttribute_Vtbl>(), size_of::<IDispatch_Vtbl>() + 3 * p);
        assert_eq!(size_of::<IHTMLDOMTextNode_Vtbl>(), size_of::<IDispatch_Vtbl>() + 2 * p);
        assert_eq!(size_of::<IHTMLElementCollection_Vtbl>(), size_of::<IDispatch_Vtbl>() + 5 * p);
        assert_eq!(size_of::<IHTMLAttributeCollection2_Vtbl>(), size_of::<IDispatch_Vtbl>() + 1 * p);
        assert_eq!(size_of::<IHTMLDOMChildrenCollection_Vtbl>(), size_of::<IDispatch_Vtbl>() + 3 * p);
        assert_eq!(size_of::<IHTMLUniqueName_Vtbl>(), size_of::<IDispatch_Vtbl>() + 1 * p);
        assert_eq!(size_of::<IHTMLElement_Vtbl>(), size_of::<IDispatch_Vtbl>() + 33 * p);
        assert_eq!(size_of::<IHTMLCurrentStyle_Vtbl>(), size_of::<IDispatch_Vtbl>() + 49 * p);
        assert_eq!(size_of::<IHTMLElement2_Vtbl>(), size_of::<IDispatch_Vtbl>() + 98 * p);
        assert_eq!(size_of::<IHTMLElement3_Vtbl>(), size_of::<IDispatch_Vtbl>() + 15 * p);
        assert_eq!(size_of::<IHTMLDocument_Vtbl>(), size_of::<IDispatch_Vtbl>() + 1 * p);
        assert_eq!(size_of::<IHTMLDocument2_Vtbl>(), size_of::<IHTMLDocument_Vtbl>() + 2 * p);
        assert_eq!(size_of::<IHTMLDocument3_Vtbl>(), size_of::<IDispatch_Vtbl>() + 40 * p);
        assert_eq!(size_of::<IMarkupPointer_Vtbl>(), size_of::<IUnknown_Vtbl>() + 14 * p);
        assert_eq!(size_of::<IMarkupContainer_Vtbl>(), size_of::<IUnknown_Vtbl>() + 1 * p);
        assert_eq!(size_of::<IMarkupContainer2_Vtbl>(), size_of::<IMarkupContainer_Vtbl>() + 4 * p);
        assert_eq!(size_of::<IMarkupServices2_Vtbl>(), size_of::<IUnknown_Vtbl>() + 1 * p);
    }
}
