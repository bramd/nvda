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
use windows::Win32::Foundation::{E_POINTER, HWND};
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
    pub role: unsafe extern "system" fn(this: *mut core::ffi::c_void, role: *mut i32) -> HRESULT,
    pub scrollTo: usize,
    pub scrollToPoint: usize,
    pub get_groupPosition: usize,
    pub get_states: unsafe extern "system" fn(this: *mut core::ffi::c_void, states: *mut i32) -> HRESULT,
    pub get_extendedRole: usize,
    pub get_localizedExtendedRole: usize,
    pub get_nExtendedStates: usize,
    pub get_extendedStates: usize,
    pub get_localizedExtendedStates: usize,
    pub get_uniqueID: unsafe extern "system" fn(this: *mut core::ffi::c_void, unique_id: *mut i32) -> HRESULT,
    pub get_windowHandle: unsafe extern "system" fn(this: *mut core::ffi::c_void, window_handle: *mut HWND) -> HRESULT,
    pub get_indexInParent: usize,
    pub get_locale: unsafe extern "system" fn(this: *mut core::ffi::c_void, locale: *mut IA2Locale) -> HRESULT,
    pub get_attributes: unsafe extern "system" fn(this: *mut core::ffi::c_void, attributes: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
}

/// Mirror of the IDL `IA2Locale` struct (Accessible2.idl:367). Three
/// server-allocated BSTRs; the caller takes ownership and is
/// responsible for `SysFreeString`. The Rust wrapper
/// [`IAccessible2::get_locale`] does this automatically.
#[repr(C)]
pub struct IA2Locale {
    pub language: core::mem::ManuallyDrop<BSTR>,
    pub country: core::mem::ManuallyDrop<BSTR>,
    pub variant: core::mem::ManuallyDrop<BSTR>,
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

    /// Returns the IA2 state bitmask. See `include/ia2/api/AccessibleStates.idl`
    /// for flag definitions (`IA2_STATE_EDITABLE`, etc.).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessible2` implementation for the duration of this
    /// call. Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_states(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_states)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the IA2 role for this object. See `IA2Role` in
    /// `include/ia2/api/AccessibleRole.idl` for the IA2-specific role
    /// values; standard MSAA `ROLE_SYSTEM_*` values are also passed
    /// through this method for convenience. The IDL names the method
    /// `role` (not `get_role`) per the comment at
    /// `include/ia2/api/Accessible2.idl:443`.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn role(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).role)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the IA2 unique ID for this object. See
    /// `include/ia2/api/Accessible2.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_uniqueID(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_uniqueID)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the host window handle for this object.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_windowHandle(&self) -> windows::core::Result<HWND> {
        let mut out = HWND::default();
        let hr = (Interface::vtable(self).get_windowHandle)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the IA2 locale (language, country, variant) for this
    /// object. All three BSTRs are server-allocated; the wrapper takes
    /// ownership of each so their `Drop` runs `SysFreeString`.
    ///
    /// Returns `None` when the call failed; we don't bother
    /// distinguishing `S_FALSE` from a hard error since neither is
    /// usable.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_locale(
        &self,
    ) -> windows::core::Result<(BSTR, BSTR, BSTR)> {
        let mut out = IA2Locale {
            language: core::mem::ManuallyDrop::new(BSTR::default()),
            country: core::mem::ManuallyDrop::new(BSTR::default()),
            variant: core::mem::ManuallyDrop::new(BSTR::default()),
        };
        let hr = (Interface::vtable(self).get_locale)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            // Take ownership of any partial BSTRs the server may have
            // written so their Drop runs SysFreeString.
            let _ = core::mem::ManuallyDrop::into_inner(out.language);
            let _ = core::mem::ManuallyDrop::into_inner(out.country);
            let _ = core::mem::ManuallyDrop::into_inner(out.variant);
            return Err(hr.into());
        }
        Ok((
            core::mem::ManuallyDrop::into_inner(out.language),
            core::mem::ManuallyDrop::into_inner(out.country),
            core::mem::ManuallyDrop::into_inner(out.variant),
        ))
    }
}

// --- IAccessible2_2 -------------------------------------------------------
//
// Inherits from IAccessible2. Vtable order (from Accessible2_2.idl):
//   1. get_attribute             -- not used yet
//   2. get_accessibleWithCaret   -- not used yet
//   3. get_relationTargetsOfType -- used by gecko_ia2 getRelationElementsOfType

windows_core::imp::define_interface!(
    IAccessible2_2,
    IAccessible2_2_Vtbl,
    0x6c9430e9_299d_4e6f_bd01_a82a1e88d3ff
);
impl core::ops::Deref for IAccessible2_2 {
    type Target = IAccessible2;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(
    IAccessible2_2,
    IUnknown,
    IAccessible,
    IAccessible2
);

#[repr(C)]
pub struct IAccessible2_2_Vtbl {
    pub base__: IAccessible2_Vtbl,
    pub get_attribute: usize,
    pub get_accessibleWithCaret: usize,
    pub get_relationTargetsOfType: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        type_: core::mem::ManuallyDrop<BSTR>,
        max_targets: i32,
        targets: *mut *mut *mut core::ffi::c_void,
        n_targets: *mut i32,
    ) -> HRESULT,
}

impl IAccessible2_2 {
    /// Returns the (server-allocated array, count) of relation targets of
    /// the given relation type. The output array is `IUnknown**`
    /// allocated with `CoTaskMemAlloc`; callers must free it via
    /// `CoTaskMemFree` and `Release` each element they keep.
    ///
    /// `S_FALSE` is treated as "no targets" (returns `(null, 0)`); the
    /// caller should not dereference a null pointer.
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessible2_2` implementation for the duration of
    /// this call. The caller is responsible for taking ownership of the
    /// returned `IUnknown` array per the IDL contract.
    pub unsafe fn get_relationTargetsOfType(
        &self,
        relation: &BSTR,
        max_targets: i32,
    ) -> windows::core::Result<(*mut *mut core::ffi::c_void, i32)> {
        let mut targets: *mut *mut core::ffi::c_void = core::ptr::null_mut();
        let mut count: i32 = 0;
        // The IDL declares `[in] BSTR type` -- the server does not take
        // ownership, but the COM ABI passes BSTRs by value with caller-
        // owned lifetime. We wrap the borrowed BSTR in ManuallyDrop so
        // it is not freed when the call returns.
        let raw_bstr =
            unsafe { core::ptr::read(relation as *const BSTR) };
        let manual = core::mem::ManuallyDrop::new(raw_bstr);
        let hr = (Interface::vtable(self).get_relationTargetsOfType)(
            Interface::as_raw(self),
            manual,
            max_targets,
            &mut targets as *mut _,
            &mut count as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((targets, count))
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
    pub get_caretOffset: unsafe extern "system" fn(this: *mut core::ffi::c_void, offset: *mut i32) -> HRESULT,
    pub get_characterExtents: usize,
    pub get_nSelections: unsafe extern "system" fn(this: *mut core::ffi::c_void, n_selections: *mut i32) -> HRESULT,
    pub get_offsetAtPoint: usize,
    pub get_selection: unsafe extern "system" fn(this: *mut core::ffi::c_void, selection_index: i32, start_offset: *mut i32, end_offset: *mut i32) -> HRESULT,
    pub get_text: unsafe extern "system" fn(this: *mut core::ffi::c_void, start_offset: i32, end_offset: i32, text: *mut core::mem::ManuallyDrop<BSTR>) -> HRESULT,
    pub get_textBeforeOffset: usize,
    pub get_textAfterOffset: usize,
    pub get_textAtOffset: usize,
    pub removeSelection: usize,
    pub setCaretOffset: usize,
    pub setSelection: usize,
    pub get_nCharacters: unsafe extern "system" fn(this: *mut core::ffi::c_void, n_characters: *mut i32) -> HRESULT,
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

    /// Returns the current caret offset within this text. See
    /// `include/ia2/api/AccessibleText.idl` for the IDL contract.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_caretOffset(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_caretOffset)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the total character count of this text.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nCharacters(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nCharacters)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the number of active text selections.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nSelections(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nSelections)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the `(startOffset, endOffset)` for the selection at
    /// `selection_index`.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_selection(
        &self,
        selection_index: i32,
    ) -> windows::core::Result<(i32, i32)> {
        let mut start: i32 = 0;
        let mut end: i32 = 0;
        let hr = (Interface::vtable(self).get_selection)(
            Interface::as_raw(self),
            selection_index,
            &mut start as *mut _,
            &mut end as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((start, end))
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
    /// Returns an `E_POINTER` error if the COM call returned `S_OK` but
    /// wrote a null hyperlink pointer (the IDL allows this for invalid
    /// indices, see `include/ia2/api/AccessibleHypertext.idl:92`).
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
            // No `ManuallyDrop::into_inner` cleanup is needed here (unlike
            // `IAccessible2::get_attributes`): `out` is
            // `Option<IAccessibleHyperlink>`, and `IAccessibleHyperlink`'s
            // `Drop` (the standard windows-rs interface drop, calling
            // `Release`) runs automatically when the `Option` falls out of
            // scope on this early-return or on the success path below.
            // Adding `into_inner` would not compile.
            return Err(hr.into());
        }
        out.ok_or_else(|| windows::core::Error::from(E_POINTER))
    }

    /// Returns the 0-based hyperlink index for the embedded-object character
    /// at `char_index`, or `-1` if the character is not on a link.
    ///
    /// The IDL contract (`include/ia2/api/AccessibleHypertext.idl`) is:
    /// returns `S_OK` with a valid index when the character is on a link,
    /// `S_FALSE` with `index = -1` when it is not. `windows::core::HRESULT`
    /// treats `S_FALSE` as success (`is_err()` is false), so this wrapper
    /// returns `Ok(-1)` in that case rather than an `Err`.
    ///
    /// **Callers must check the returned value is `>= 0` before passing it
    /// to `get_hyperlink`** -- passing `-1` would result in a wasted COM
    /// call returning `E_INVALIDARG`.
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

impl IAccessibleHypertext2 {
    /// Returns the (server-allocated array, count) of hyperlinks on this
    /// hypertext. Each `Option<IAccessibleHyperlink>` in the array is
    /// AddRef'd; the caller owns them. The caller is also responsible
    /// for freeing the outer array via
    /// `windows::Win32::System::Com::CoTaskMemFree`.
    ///
    /// On error, returns `Err(hr)` and the out-params are not written.
    /// On success with zero links, returns `Ok((null, 0))` -- the caller
    /// should not dereference the array but should still skip the free
    /// (CoTaskMemFree on null is documented as a no-op, so calling it
    /// either way is fine).
    ///
    /// # Safety
    ///
    /// The underlying COM pointer wrapped by `self` must point to a live,
    /// well-formed `IAccessibleHypertext2` implementation for the duration
    /// of this call.
    pub unsafe fn get_hyperlinks(
        &self,
    ) -> windows::core::Result<(*mut Option<IAccessibleHyperlink>, i32)> {
        let mut ptr: *mut Option<IAccessibleHyperlink> = core::ptr::null_mut();
        let mut count: i32 = 0;
        let hr = (Interface::vtable(self).get_hyperlinks)(
            Interface::as_raw(self),
            &mut ptr as *mut _,
            &mut count as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((ptr, count))
    }
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

// --- IAccessibleTable2 ----------------------------------------------------
//
// Inherits from IUnknown. Vtable order (from AccessibleTable2.idl):
//   1.  get_cellAt
//   2.  get_caption
//   3.  get_columnDescription
//   4.  get_nColumns          <-- used
//   5.  get_nRows             <-- used
//   6.  get_nSelectedCells
//   7.  get_nSelectedColumns
//   8.  get_nSelectedRows
//   9.  get_rowDescription
//   10. get_selectedCells
//   11. get_selectedColumns
//   12. get_selectedRows
//   13. get_summary
//   14. get_isColumnSelected
//   15. get_isRowSelected
//   16. selectRow
//   17. selectColumn
//   18. unselectRow
//   19. unselectColumn
//   20. get_modelChange

windows_core::imp::define_interface!(
    IAccessibleTable2,
    IAccessibleTable2_Vtbl,
    0x6167f295_06f0_4cdd_a1fa_02e25153d869
);
impl core::ops::Deref for IAccessibleTable2 {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleTable2, IUnknown);

#[repr(C)]
pub struct IAccessibleTable2_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub get_cellAt: usize,
    pub get_caption: usize,
    pub get_columnDescription: usize,
    pub get_nColumns: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        column_count: *mut i32,
    ) -> HRESULT,
    pub get_nRows: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        row_count: *mut i32,
    ) -> HRESULT,
    // The remaining slots (get_nSelectedCells through get_modelChange)
    // are not exercised yet; declared as opaque placeholders to keep
    // the vtable layout correct when callers cast a server's vptr.
}

impl IAccessibleTable2 {
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nColumns(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nColumns)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_nRows(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).get_nRows)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }
}

// --- IAccessibleTableCell -------------------------------------------------
//
// Inherits from IUnknown. Vtable order (from AccessibleTableCell.idl):
//   1. get_columnExtent
//   2. get_columnHeaderCells     <-- used (table header walk)
//   3. get_columnIndex
//   4. get_rowExtent
//   5. get_rowHeaderCells        <-- used (table header walk)
//   6. get_rowIndex
//   7. get_isSelected
//   8. get_rowColumnExtents      <-- used (cell info)
//   9. get_table                 <-- used (table-id lookup)

windows_core::imp::define_interface!(
    IAccessibleTableCell,
    IAccessibleTableCell_Vtbl,
    0x594116b1_c99f_4847_ad06_0a7a86ece645
);
impl core::ops::Deref for IAccessibleTableCell {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleTableCell, IUnknown);

#[repr(C)]
pub struct IAccessibleTableCell_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub get_columnExtent: usize,
    pub get_columnHeaderCells: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        cell_accessibles: *mut *mut *mut core::ffi::c_void,
        n_column_header_cells: *mut i32,
    ) -> HRESULT,
    pub get_columnIndex: usize,
    pub get_rowExtent: usize,
    pub get_rowHeaderCells: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        cell_accessibles: *mut *mut *mut core::ffi::c_void,
        n_row_header_cells: *mut i32,
    ) -> HRESULT,
    pub get_rowIndex: usize,
    pub get_isSelected: usize,
    pub get_rowColumnExtents: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        row: *mut i32,
        column: *mut i32,
        row_extents: *mut i32,
        column_extents: *mut i32,
        is_selected: *mut u8,
    ) -> HRESULT,
    pub get_table: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        table: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
}

/// Result of `IAccessibleTableCell::get_rowColumnExtents`. The IDL
/// names the boolean `isSelected` but the IA2 servers we deal with
/// pass it as a `BOOLEAN` (8-bit) so we expose it as `bool` for
/// ergonomics.
#[derive(Clone, Copy, Debug)]
pub struct RowColumnExtents {
    pub row: i32,
    pub column: i32,
    pub row_extents: i32,
    pub column_extents: i32,
    pub is_selected: bool,
}

impl IAccessibleTableCell {
    /// Returns the array of header `IUnknown*` pointers for one axis
    /// (column or row). The array is `CoTaskMemAlloc`'d; the caller
    /// must `CoTaskMemFree` it and `Release` each entry.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_column_header_cells(
        &self,
    ) -> windows::core::Result<(*mut *mut core::ffi::c_void, i32)> {
        let mut cells: *mut *mut core::ffi::c_void = core::ptr::null_mut();
        let mut count: i32 = 0;
        let hr = (Interface::vtable(self).get_columnHeaderCells)(
            Interface::as_raw(self),
            &mut cells as *mut _,
            &mut count as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((cells, count))
    }

    /// See [`IAccessibleTableCell::get_column_header_cells`].
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_row_header_cells(
        &self,
    ) -> windows::core::Result<(*mut *mut core::ffi::c_void, i32)> {
        let mut cells: *mut *mut core::ffi::c_void = core::ptr::null_mut();
        let mut count: i32 = 0;
        let hr = (Interface::vtable(self).get_rowHeaderCells)(
            Interface::as_raw(self),
            &mut cells as *mut _,
            &mut count as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok((cells, count))
    }

    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_row_column_extents(
        &self,
    ) -> windows::core::Result<RowColumnExtents> {
        let mut row: i32 = 0;
        let mut column: i32 = 0;
        let mut row_extents: i32 = 0;
        let mut column_extents: i32 = 0;
        let mut is_selected: u8 = 0;
        let hr = (Interface::vtable(self).get_rowColumnExtents)(
            Interface::as_raw(self),
            &mut row,
            &mut column,
            &mut row_extents,
            &mut column_extents,
            &mut is_selected,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(RowColumnExtents {
            row,
            column,
            row_extents,
            column_extents,
            is_selected: is_selected != 0,
        })
    }

    /// Returns the parent `IUnknown*` (which servers usually expose as
    /// an `IAccessibleTable2` and clients QI to `IAccessible2`). The
    /// caller takes ownership of the returned reference.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_table(&self) -> windows::core::Result<IUnknown> {
        let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).get_table)(
            Interface::as_raw(self),
            &mut raw as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        if raw.is_null() {
            return Err(E_POINTER.into());
        }
        Ok(unsafe { IUnknown::from_raw(raw) })
    }
}

// --- IAccessibleAction ----------------------------------------------------
//
// Inherits from IUnknown. Vtable order (from AccessibleAction.idl):
//   1. nActions               <-- used (default action exposure)
//   2. doAction
//   3. get_description
//   4. get_keyBinding
//   5. get_name               <-- used (default action exposure)
//   6. get_localizedName

windows_core::imp::define_interface!(
    IAccessibleAction,
    IAccessibleAction_Vtbl,
    0xb70d9f59_3b5a_4dba_ab9e_22012f607df5
);
impl core::ops::Deref for IAccessibleAction {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleAction, IUnknown);

#[repr(C)]
pub struct IAccessibleAction_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub nActions: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        n_actions: *mut i32,
    ) -> HRESULT,
    pub doAction: usize,
    pub get_description: usize,
    pub get_keyBinding: usize,
    pub get_name: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        action_index: i32,
        name: *mut core::mem::ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub get_localizedName: usize,
}

impl IAccessibleAction {
    /// Number of actions exposed by the object.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn nActions(&self) -> windows::core::Result<i32> {
        let mut out: i32 = 0;
        let hr = (Interface::vtable(self).nActions)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            return Err(hr.into());
        }
        Ok(out)
    }

    /// Returns the (non-localized) name of the action at `index`.
    /// Server-allocated BSTR; the wrapper takes ownership.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_name(&self, index: i32) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_name)(
            Interface::as_raw(self),
            index,
            &mut out as *mut _,
        );
        if hr.is_err() {
            let _ = core::mem::ManuallyDrop::into_inner(out);
            return Err(hr.into());
        }
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }
}

// --- IAccessibleApplication -----------------------------------------------
//
// Inherits from IUnknown. Vtable order (from AccessibleApplication.idl):
//   1. get_appName
//   2. get_appVersion
//   3. get_toolkitName    <-- the only one we use
//   4. get_toolkitVersion

windows_core::imp::define_interface!(
    IAccessibleApplication,
    IAccessibleApplication_Vtbl,
    0xd49ded83_5b25_43f4_9b95_93b44595979e
);
impl core::ops::Deref for IAccessibleApplication {
    type Target = IUnknown;
    fn deref(&self) -> &Self::Target {
        unsafe { core::mem::transmute(self) }
    }
}
windows_core::imp::interface_hierarchy!(IAccessibleApplication, IUnknown);

#[repr(C)]
pub struct IAccessibleApplication_Vtbl {
    pub base__: IUnknown_Vtbl,
    pub get_appName: usize,
    pub get_appVersion: usize,
    pub get_toolkitName: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        name: *mut core::mem::ManuallyDrop<BSTR>,
    ) -> HRESULT,
    pub get_toolkitVersion: usize,
}

impl IAccessibleApplication {
    /// Returns the toolkit name (e.g. `"Mozilla Gecko"`, `"Chrome"`).
    /// Server-allocated BSTR; the wrapper takes ownership.
    ///
    /// # Safety
    ///
    /// Same apartment / lifetime obligations as
    /// [`IAccessible2::get_attributes`].
    pub unsafe fn get_toolkitName(&self) -> windows::core::Result<BSTR> {
        let mut out = core::mem::ManuallyDrop::new(BSTR::default());
        let hr = (Interface::vtable(self).get_toolkitName)(
            Interface::as_raw(self),
            &mut out as *mut _,
        );
        if hr.is_err() {
            let _ = core::mem::ManuallyDrop::into_inner(out);
            return Err(hr.into());
        }
        Ok(core::mem::ManuallyDrop::into_inner(out))
    }
}
