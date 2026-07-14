//! Phase 0 spike for the UIA event rate-limiter port.
//!
//! Proves that windows-rs `#[implement]` can implement all five
//! `IUIAutomation*EventHandler` COM interfaces (the receive side of
//! `RateLimitedEventHandler`) and that the resulting staticlib links into
//! nvdaHelperLocal.dll. The real coalescing queue, flusher thread, and the
//! `rateLimitedUIAEventHandler_create`/`_terminate` C ABI land in later
//! phases; here the handler methods are empty stubs.

#![allow(non_snake_case)]

pub mod dedup;
pub mod event;

use windows::core::{implement, Result, BSTR, VARIANT};
use windows::Win32::UI::Accessibility::{
    IUIAutomationActiveTextPositionChangedEventHandler,
    IUIAutomationActiveTextPositionChangedEventHandler_Impl, IUIAutomationElement,
    IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
    IUIAutomationFocusChangedEventHandler,
    IUIAutomationFocusChangedEventHandler_Impl,
    IUIAutomationNotificationEventHandler,
    IUIAutomationNotificationEventHandler_Impl,
    IUIAutomationPropertyChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler_Impl, IUIAutomationTextRange,
    NotificationKind, NotificationProcessing, UIA_EVENT_ID, UIA_PROPERTY_ID,
};

/// The rate-limited UIA event handler. In Phase 0 it holds no state and
/// every handler is a no-op; it exists only to prove the `#[implement]` of
/// all five interfaces compiles and links.
#[implement(
    IUIAutomationEventHandler,
    IUIAutomationFocusChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler,
    IUIAutomationNotificationEventHandler,
    IUIAutomationActiveTextPositionChangedEventHandler
)]
struct RateLimitedEventHandler;

impl IUIAutomationEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleAutomationEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
        _eventid: UIA_EVENT_ID,
    ) -> Result<()> {
        Ok(())
    }
}

impl IUIAutomationFocusChangedEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
    ) -> Result<()> {
        Ok(())
    }
}

impl IUIAutomationPropertyChangedEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
        _propertyid: UIA_PROPERTY_ID,
        _newvalue: &VARIANT,
    ) -> Result<()> {
        Ok(())
    }
}

impl IUIAutomationNotificationEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleNotificationEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
        _notificationkind: NotificationKind,
        _notificationprocessing: NotificationProcessing,
        _displaystring: &BSTR,
        _activityid: &BSTR,
    ) -> Result<()> {
        Ok(())
    }
}

impl IUIAutomationActiveTextPositionChangedEventHandler_Impl
    for RateLimitedEventHandler_Impl
{
    fn HandleActiveTextPositionChangedEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
        _range: Option<&IUIAutomationTextRange>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Phase 0 link probe: a referenced C symbol so the staticlib is pulled
/// into nvdaHelperLocal.dll and the `#[implement]` object above is
/// instantiated (verifying it is a complete, constructible COM object).
/// Replaced by the real `rateLimitedUIAEventHandler_create`/`_terminate`
/// exports in Phase 2.
#[no_mangle]
pub extern "C" fn nvda_uia_events_probe() -> i32 {
    let handler: IUIAutomationEventHandler = RateLimitedEventHandler.into();
    // Keep the COM object alive across a trivial use so the optimizer can't
    // elide the construction path.
    core::mem::drop(handler);
    0
}
