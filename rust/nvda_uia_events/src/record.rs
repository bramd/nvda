//! The queued event payload and the emit side.
//!
//! A `Record` is what the dedup queue holds: everything needed to re-emit a
//! UIA event to the wrapped ("existing") handler. It carries live COM
//! objects (`IUIAutomationElement` / `IUIAutomationTextRange` / `VARIANT` /
//! `BSTR`). Those are moved from the UIA callback thread, through the queue,
//! to the flusher thread — so `Record` and `ExistingHandlers` need to cross
//! threads, but windows-rs COM types are `!Send`.
//!
//! D2 (see the port plan): we assert `Send` via `unsafe impl`. This is
//! faithful to the shipping C++, which moves these same pointers between its
//! callback and flusher threads with no marshaling, relying on UIA objects
//! being **agile** (free-threaded). The alternative — `AgileReference` —
//! would marshal correctly across apartments but changes behavior and adds
//! per-object cost; we keep the faithful, zero-overhead assertion.

use core::ffi::c_void;

use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound,
    SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationEventHandler,
    IUIAutomationFocusChangedEventHandler,
    IUIAutomationNotificationEventHandler,
    IUIAutomationPropertyChangedEventHandler, IUIAutomationTextRange,
    IUIAutomationActiveTextPositionChangedEventHandler, NotificationKind,
    NotificationProcessing, UIA_EVENT_ID, UIA_PROPERTY_ID,
};

/// A queued UIA event, holding the COM data to re-emit. One variant per
/// handled interface (the C++ `EventRecordVariant_t`).
pub enum Record {
    Automation {
        sender: IUIAutomationElement,
        event_id: UIA_EVENT_ID,
    },
    FocusChanged {
        sender: IUIAutomationElement,
    },
    PropertyChanged {
        sender: IUIAutomationElement,
        property_id: UIA_PROPERTY_ID,
        new_value: VARIANT,
    },
    Notification {
        sender: IUIAutomationElement,
        kind: NotificationKind,
        processing: NotificationProcessing,
        display_string: BSTR,
        activity_id: BSTR,
    },
    ActiveTextPositionChanged {
        sender: IUIAutomationElement,
        /// Nullable, as the C++ stores a possibly-NULL `CComPtr`.
        range: Option<IUIAutomationTextRange>,
    },
}

// SAFETY: see the module doc — the UIA objects carried here are agile, and
// the shipping C++ already moves them across the same thread boundary.
unsafe impl Send for Record {}

/// The wrapped "existing" handler, queried for each of the five interfaces
/// (any of which may be absent). In production this is NVDA's Python
/// `UIAHandler` COMObject; `emit` makes COM calls back into it.
pub struct ExistingHandlers {
    automation: Option<IUIAutomationEventHandler>,
    focus: Option<IUIAutomationFocusChangedEventHandler>,
    property: Option<IUIAutomationPropertyChangedEventHandler>,
    notification: Option<IUIAutomationNotificationEventHandler>,
    active_text: Option<IUIAutomationActiveTextPositionChangedEventHandler>,
}

// SAFETY: as with Record; the handlers are owned by (and only used on) the
// single flusher thread, matching the C++ CComQIPtr members.
unsafe impl Send for ExistingHandlers {}

impl ExistingHandlers {
    /// Query the wrapped handler for each interface (mirrors the C++ ctor's
    /// five `CComQIPtr` initialisations off the one `IUnknown`).
    pub fn from_unknown(existing: &windows::core::IUnknown) -> Self {
        Self {
            automation: existing.cast().ok(),
            focus: existing.cast().ok(),
            property: existing.cast().ok(),
            notification: existing.cast().ok(),
            active_text: existing.cast().ok(),
        }
    }

    /// Emit one record to the corresponding existing handler. Errors are
    /// swallowed (the C++ logs and returns the HRESULT; there is nothing to
    /// do with a failed forward here).
    pub fn emit(&self, record: Record) {
        match record {
            Record::Automation { sender, event_id } => {
                if let Some(h) = &self.automation {
                    let _ =
                        unsafe { h.HandleAutomationEvent(&sender, event_id) };
                }
            }
            Record::FocusChanged { sender } => {
                if let Some(h) = &self.focus {
                    let _ = unsafe { h.HandleFocusChangedEvent(&sender) };
                }
            }
            Record::PropertyChanged {
                sender,
                property_id,
                new_value,
            } => {
                if let Some(h) = &self.property {
                    let _ = unsafe {
                        h.HandlePropertyChangedEvent(
                            &sender,
                            property_id,
                            &new_value,
                        )
                    };
                }
            }
            Record::Notification {
                sender,
                kind,
                processing,
                display_string,
                activity_id,
            } => {
                if let Some(h) = &self.notification {
                    let _ = unsafe {
                        h.HandleNotificationEvent(
                            &sender,
                            kind,
                            processing,
                            &display_string,
                            &activity_id,
                        )
                    };
                }
            }
            Record::ActiveTextPositionChanged { sender, range } => {
                if let Some(h) = &self.active_text {
                    let _ = unsafe {
                        h.HandleActiveTextPositionChangedEvent(
                            &sender,
                            range.as_ref(),
                        )
                    };
                }
            }
        }
    }
}

/// Fetch an element's UIA RuntimeId as a `Vec<i32>` (the coalescing-key
/// prefix). Port of `getRuntimeIDFromElement` + `SafeArrayToVector`
/// (utils.cpp); returns empty on any failure.
pub fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    let sa = match unsafe { element.GetRuntimeId() } {
        Ok(sa) if !sa.is_null() => sa,
        _ => return Vec::new(),
    };
    let out = unsafe { safearray_to_vec(sa) };
    unsafe {
        let _ = SafeArrayDestroy(sa);
    }
    out
}

/// Copy a 1-D `SAFEARRAY` of `i32` into a `Vec<i32>`. Mirrors
/// `SafeArrayToVector`.
///
/// # Safety
///
/// `sa` must be a valid 1-D `SAFEARRAY` of `VT_I4`.
unsafe fn safearray_to_vec(sa: *const SAFEARRAY) -> Vec<i32> {
    let mut data: *mut c_void = core::ptr::null_mut();
    if unsafe { SafeArrayAccessData(sa, &mut data) }.is_err() {
        return Vec::new();
    }
    let lower = unsafe { SafeArrayGetLBound(sa, 1) }.unwrap_or(0);
    let upper = unsafe { SafeArrayGetUBound(sa, 1) }.unwrap_or(-1);
    let len = (upper - lower + 1).max(0) as usize;
    let out = unsafe { core::slice::from_raw_parts(data as *const i32, len) }
        .to_vec();
    let _ = unsafe { SafeArrayUnaccessData(sa) };
    out
}
