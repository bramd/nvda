//! Hand-rolled bindings for the IAccessible2 COM interfaces this project
//! consumes. Method orderings come from the corresponding IDL files in
//! `include/ia2/api/`. Only the methods used (or expected to be used by the
//! follow-up PR) are declared — the vtable trailing slots we don't need are
//! filled with `unused: usize` to keep the offsets correct.
//!
//! IID quick reference (verbatim from the IDLs):
//! - IAccessible2:           E89F726E-C4F4-4c19-BB19-B647D7FA8478  (Accessible2.idl:383)
//! - IAccessibleText:        24FD2FFB-3AAD-4a08-8335-A3AD89C0FB4B  (AccessibleText.idl)
//! - IAccessibleHypertext:   6B4F8BBF-F1F2-418a-B35E-A195BC4103B9  (AccessibleHypertext.idl)
//! - IAccessibleHypertext2:  CF64D89F-8287-4B44-8501-A827453A6077  (AccessibleHypertext2.idl)
//! - IAccessibleHyperlink:   01C20F2B-3DD2-400f-949F-AD00BDAB1D41  (AccessibleHyperlink.idl)
//!
//! Implementation note: `windows-core` 0.58 exposes `interface!` only as the
//! `#[interface(...)]` attribute proc-macro on `unsafe trait` declarations,
//! which auto-generates the `_Vtbl` struct and method wrappers. To preserve
//! hand-rolled control over the vtable layout (so we can use `usize`
//! placeholders for unused slots), we instead use the same lower-level
//! `define_interface!` + `interface_hierarchy!` pattern that windows-rs uses
//! internally for its own generated bindings (see e.g. `IAccessibleEx` in
//! `windows::Win32::UI::Accessibility`).

use windows::core::{BSTR, HRESULT, IUnknown, IUnknown_Vtbl, Interface};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::UI::Accessibility::{IAccessible, IAccessible_Vtbl};

use crate::types::IA2TextSegment;

// --- IAccessible2 ---------------------------------------------------------
//
// Inherits from IAccessible. We declare only the trailing slots we need
// (get_attributes is the last method in the vtable, so we must list every
// method between the IAccessible base and get_attributes to preserve vtable
// offsets). For PR 1 we only call get_attributes; the slots before it are
// declared as opaque `unused` raw pointers.
//
// Vtable order (from Accessible2.idl, after the IAccessible methods):
//   1.  get_nRelations
//   2.  get_relation
//   3.  get_relations
//   4.  role
//   5.  scrollTo
//   6.  scrollToPoint
//   7.  get_groupPosition
//   8.  get_states
//   9.  get_extendedRole
//   10. get_localizedExtendedRole
//   11. get_nExtendedStates
//   12. get_extendedStates
//   13. get_localizedExtendedStates
//   14. get_uniqueID
//   15. get_windowHandle
//   16. get_indexInParent
//   17. get_locale
//   18. get_attributes  <-- the only one we use

windows_core::imp::define_interface!(
    IAccessible2,
    IAccessible2_Vtbl,
    0xe89f726e_c4f4_4c19_bb19_b647d7fa8478
);
impl core::ops::Deref for IAccessible2 {
    type Target = IAccessible;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessible2, IUnknown, IAccessible);

#[repr(C)]
pub struct IAccessible2_Vtbl {
    pub base__: IAccessible_Vtbl,
    pub get_nRelations: usize,
    pub get_relation: usize,
    pub get_relations: usize,
    pub role: usize,
    pub scrollTo: usize,
    pub scrollToPoint: usize,
    pub get_groupPosition: usize,
    pub get_states: usize,
    pub get_extendedRole: usize,
    pub get_localizedExtendedRole: usize,
    pub get_nExtendedStates: usize,
    pub get_extendedStates: usize,
    pub get_localizedExtendedStates: usize,
    pub get_uniqueID: usize,
    pub get_windowHandle: usize,
    pub get_indexInParent: usize,
    pub get_locale: usize,
    pub get_attributes: unsafe extern "system" fn(this: *mut core::ffi::c_void, attributes: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
}

impl IAccessible2 {
    /// Returns the IA2 attributes string (server-allocated BSTR).
    /// Returns `S_FALSE` with a NULL output BSTR if there are no attributes
    /// (per the IDL contract at Accessible2.idl:687).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessible2` implementation for the duration of this
    /// call. The `&self` borrow encodes this in Rust terms, but COM objects
    /// can be in invalid states across thread or process boundaries (e.g.
    /// after the server has been torn down, or when called from a thread
    /// the object was not marshalled to). Callers are responsible for
    /// ensuring the apartment / lifetime invariants are upheld.
    pub unsafe fn get_attributes(&self) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_attributes)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            // Take ownership of any BSTR a misbehaving server may have written
            // before returning failure, so its Drop runs SysFreeString.
            let _ = core::mem::ManuallyDrop::into_inner(out);
            return Err(hr.into());
        }
        // Take ownership of the BSTR (BSTR's Drop will SysFreeString).
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }
}

// --- IAccessibleText ------------------------------------------------------
//
// PR 2 will use get_text and get_newText. We declare the full prefix of the
// vtable up to (and including) get_newText. Until PR 2 wires it up, the
// methods are present but unexercised — Rust function wrappers will be added
// in PR 2 when callers need them. The vtable layout (the `_Vtbl` struct's
// field order and widths) must include every slot up to the last one we care
// about so that offset arithmetic for the methods we DO call lands at the
// correct vtable index. Note that `_Vtbl` is a *description* of the COM
// server's vtable, not a vtable our code constructs.
//
// Vtable order (from AccessibleText.idl):
//   1.  addSelection
//   2.  get_attributes
//   3.  get_caretOffset
//   4.  get_characterExtents
//   5.  get_nSelections
//   6.  get_offsetAtPoint
//   7.  get_selection
//   8.  get_text
//   9.  get_textBeforeOffset
//   10. get_textAfterOffset
//   11. get_textAtOffset
//   12. removeSelection
//   13. setCaretOffset
//   14. setSelection
//   15. get_nCharacters
//   16. scrollSubstringTo
//   17. scrollSubstringToPoint
//   18. get_newText
//   19. get_oldText

windows_core::imp::define_interface!(
    IAccessibleText,
    IAccessibleText_Vtbl,
    0x24fd2ffb_3aad_4a08_8335_a3ad89c0fb4b
);
impl core::ops::Deref for IAccessibleText {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleText, IUnknown);

#[repr(C)]
pub struct IAccessibleText_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub addSelection: usize,
    pub get_attributes: usize,
    pub get_caretOffset: usize,
    pub get_characterExtents: usize,
    pub get_nSelections: usize,
    pub get_offsetAtPoint: usize,
    pub get_selection: usize,
    pub get_text: unsafe extern "system" fn(this: *mut core::ffi::c_void, start_offset: i32, end_offset: i32, text: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
    pub get_textBeforeOffset: usize,
    pub get_textAfterOffset: usize,
    pub get_textAtOffset: usize,
    pub removeSelection: usize,
    pub setCaretOffset: usize,
    pub setSelection: usize,
    pub get_nCharacters: usize,
    pub scrollSubstringTo: usize,
    pub scrollSubstringToPoint: usize,
    pub get_newText: unsafe extern "system" fn(this: *mut core::ffi::c_void, new_text: *mut IA2TextSegment) -> HRESULT,
    pub get_oldText: usize,
}

impl IAccessibleText {
    /// Returns `[start_offset, end_offset)` of the text. Pass `0` /
    /// `IA2_TEXT_OFFSET_LENGTH` (-1) to retrieve the whole string.
    /// See `include/ia2/api/AccessibleText.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleText` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_text(&self, start_offset: i32, end_offset: i32) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_text)(
            Interface::as_raw(self),
            start_offset,
            end_offset,
            &mut out as *mut _,
        );
        if hr.is_err() {
            // Drop any BSTR a misbehaving server may have written before
            // returning failure.
            let _ = core::mem::ManuallyDrop::into_inner(out);
            return Err(hr.into());
        }
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }

    /// Returns the most recently inserted text segment for this object.
    /// Only valid during an in-process winEvent callback (IA2_EVENT_TEXT_*).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleText` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`]. The returned `IA2TextSegment` owns
    /// its `text` BSTR (freed on drop).
    pub unsafe fn get_newText(&self) -> windows::core::Result<IA2TextSegment> {
        let mut out = IA2TextSegment::default();
        let hr = (Interface::vtable(self).get_newText)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            // The compiler-generated drop for `IA2TextSegment` calls
            // `BSTR::Drop` on the `text` field, which calls SysFreeString
            // on any non-null pointer a misbehaving server may have
            // written before returning failure.
            return Err(hr.into());
        }
        Ok(out)
    }
}

// --- IAccessibleHypertext -------------------------------------------------
//
// Inherits from IAccessibleText. Vtable order (from AccessibleHypertext.idl):
//   1. get_nHyperlinks
//   2. get_hyperlink
//   3. get_hyperlinkIndex

windows_core::imp::define_interface!(
    IAccessibleHypertext,
    IAccessibleHypertext_Vtbl,
    0x6b4f8bbf_f1f2_418a_b35e_a195bc4103b9
);
impl core::ops::Deref for IAccessibleHypertext {
    type Target = IAccessibleText;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleHypertext, IUnknown, IAccessibleText);

#[repr(C)]
pub struct IAccessibleHypertext_Vtbl {
    pub base__: IAccessibleText_Vtbl,
    pub get_nHyperlinks: usize,
    pub get_hyperlink: unsafe extern "system" fn(this: *mut core::ffi::c_void, index: i32, hyperlink: *mut Option<IAccessibleHyperlink>) -> HRESULT,
    pub get_hyperlinkIndex: unsafe extern "system" fn(this: *mut core::ffi::c_void, char_index: i32, hyperlink_index: *mut i32) -> HRESULT,
}

impl IAccessibleHypertext {
    /// Retrieves the hyperlink at `index`. The COM contract returns
    /// `E_INVALIDARG` when `index >= n_hyperlinks`. The caller is expected
    /// to bound-check via `get_hyperlinkIndex` first (the pattern used by
    /// `getTextFromIAccessible`).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext` implementation for the duration of
    /// this call.
    pub unsafe fn get_hyperlink(&self, index: i32) -> windows::core::Result<IAccessibleHyperlink> {
        let mut out: Option<IAccessibleHyperlink> = None;
        let hr = (Interface::vtable(self).get_hyperlink)(
            Interface::as_raw(self),
            index,
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        out.ok_or_else(|| windows::core::Error::from(E_POINTER))
    }

    /// Returns the hyperlink index for the embedded-object character at
    /// `char_index`, or an HRESULT error if there is no hyperlink at that
    /// offset.
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext` implementation for the duration of
    /// this call.
    pub unsafe fn get_hyperlinkIndex(&self, char_index: i32) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_hyperlinkIndex)(
            Interface::as_raw(self),
            char_index,
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }
}

// --- IAccessibleHypertext2 ------------------------------------------------
//
// Inherits from IAccessibleHypertext. Vtable order (AccessibleHypertext2.idl):
//   1. get_hyperlinks  -- BSTRs allocated by server with CoTaskMemAlloc;
//                          client frees with CoTaskMemFree.

windows_core::imp::define_interface!(
    IAccessibleHypertext2,
    IAccessibleHypertext2_Vtbl,
    0xcf64d89f_8287_4b44_8501_a827453a6077
);
impl core::ops::Deref for IAccessibleHypertext2 {
    type Target = IAccessibleHypertext;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(
    IAccessibleHypertext2,
    IUnknown,
    IAccessibleText,
    IAccessibleHypertext
);

#[repr(C)]
pub struct IAccessibleHypertext2_Vtbl {
    pub base__: IAccessibleHypertext_Vtbl,
    pub get_hyperlinks: unsafe extern "system" fn(this: *mut core::ffi::c_void, hyperlinks: *mut *mut Option<IAccessibleHyperlink>, n_hyperlinks: *mut i32) -> HRESULT,
}

// --- IAccessibleHyperlink -------------------------------------------------
//
// Inherits from IAccessibleAction in the IDL, but PR 2 only QIs to it and
// doesn't call its methods directly (it's QI'd to IAccessible2). Declaring
// IUnknown as the parent here lets us get an IID-typed wrapper without
// pulling in the IAccessibleAction binding. PR 2 should not need to revisit
// this unless a future caller actually invokes hyperlink methods.

windows_core::imp::define_interface!(
    IAccessibleHyperlink,
    IAccessibleHyperlink_Vtbl,
    0x01c20f2b_3dd2_400f_949f_ad00bdab1d41
);
impl core::ops::Deref for IAccessibleHyperlink {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleHyperlink, IUnknown);

#[repr(C)]
pub struct IAccessibleHyperlink_Vtbl {
    pub base__: IUnknown_Vtbl,
    // Methods deliberately omitted -- this PR only needs the IID for QI.
}
