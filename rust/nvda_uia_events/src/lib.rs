//! Rust port of NVDA's UIA event rate-limiter
//! (`nvdaHelper/local/UIAEventLimiter/`).
//!
//! A COM object that NVDA registers with UI Automation as the handler for
//! every event type. It coalesces/de-duplicates the incoming event storm in
//! an insertion-ordered queue and forwards the survivors, on a dedicated
//! flusher thread, to the wrapped "existing" handler (NVDA's Python
//! `UIAHandler` COMObject) — so UIA core never blocks on Python.
//!
//! The COM object is built with windows-rs `#[implement]`; the two C exports
//! (`rateLimitedUIAEventHandler_create` / `_terminate`) preserve the ABI the
//! Python side calls via ctypes. The dedup core lives in [`dedup`] +
//! [`event`]; the emit side + COM payload in [`record`].

#![allow(non_snake_case)]

pub mod dedup;
pub mod event;
pub mod record;

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;

use windows::core::{implement, IUnknown, Interface, Result, BSTR, VARIANT};
use windows::Win32::Foundation::{E_INVALIDARG, S_OK};
use windows::Win32::UI::Accessibility::{
    IUIAutomationActiveTextPositionChangedEventHandler,
    IUIAutomationActiveTextPositionChangedEventHandler_Impl,
    IUIAutomationElement, IUIAutomationEventHandler,
    IUIAutomationEventHandler_Impl, IUIAutomationFocusChangedEventHandler,
    IUIAutomationFocusChangedEventHandler_Impl,
    IUIAutomationNotificationEventHandler,
    IUIAutomationNotificationEventHandler_Impl,
    IUIAutomationPropertyChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler_Impl, IUIAutomationTextRange,
    NotificationKind, NotificationProcessing, UIA_EVENT_ID, UIA_PROPERTY_ID,
};

use crate::dedup::OrderedDedup;
use crate::event::EventKind;
use crate::record::{runtime_id, ExistingHandlers, Record};

/// Cross-thread state shared by the COM handler (which enqueues) and the
/// flusher thread (which drains + emits).
struct Shared {
    inner: Mutex<Inner>,
    cond: Condvar,
}

struct Inner {
    queue: OrderedDedup<Record>,
    /// Set when the queue goes non-empty; the flusher clears it after a
    /// drain. Mirrors the C++ `m_needsFlush`.
    needs_flush: bool,
    /// Set by `terminate` to wind the flusher down.
    stop: bool,
}

impl Shared {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: OrderedDedup::new(),
                needs_flush: false,
                stop: false,
            }),
            cond: Condvar::new(),
        }
    }

    /// Queue an event, requesting a flush only when the queue was empty
    /// (the flusher drains everything once woken) — mirrors `queueEvent`.
    fn enqueue(&self, key: Vec<i32>, record: Record) {
        let mut inner = self.inner.lock().unwrap();
        let was_empty = inner.queue.is_empty();
        inner.queue.insert(key, record);
        if was_empty {
            inner.needs_flush = true;
            drop(inner);
            self.cond.notify_one();
        }
    }

    /// Ask the flusher to stop after draining anything pending.
    fn request_stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.stop = true;
        drop(inner);
        self.cond.notify_one();
    }
}

/// The flusher loop. Sleeps until an event is queued or a stop is
/// requested; drains the whole queue under the lock, then emits **outside**
/// the lock (so enqueues never block on Python). Exits after flushing when
/// stop is requested. Port of `flusherThreadFunc` + `flushEvents`.
fn run_flusher(shared: Arc<Shared>, handlers: ExistingHandlers) {
    loop {
        let (batch, stop) = {
            let mut inner = shared.inner.lock().unwrap();
            while !inner.needs_flush && !inner.stop {
                inner = shared.cond.wait(inner).unwrap();
            }
            let batch = if inner.needs_flush {
                inner.needs_flush = false;
                inner.queue.drain()
            } else {
                Vec::new()
            };
            (batch, inner.stop)
        };
        for record in batch {
            handlers.emit(record);
        }
        if stop {
            break;
        }
    }
    // `handlers` drops here, releasing the wrapped handler's COM refs on
    // this (the flusher) thread.
}

/// The rate-limited UIA event handler COM object. Holds only the shared
/// queue state; the flusher thread and the wrapped handler live off in the
/// spawned thread, and the `JoinHandle` lives in the registry — so this
/// object never has a strong reference to its own flusher (which would be a
/// refcount cycle).
#[implement(
    IUIAutomationEventHandler,
    IUIAutomationFocusChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler,
    IUIAutomationNotificationEventHandler,
    IUIAutomationActiveTextPositionChangedEventHandler
)]
struct RateLimitedEventHandler {
    shared: Arc<Shared>,
}

impl RateLimitedEventHandler {
    /// Build the coalescing key from the sender's RuntimeId + the event
    /// kind, then queue the record. A NULL sender (never sent by UIA in
    /// practice) is dropped.
    fn enqueue(
        &self,
        sender: Option<&IUIAutomationElement>,
        kind: EventKind,
        make_record: impl FnOnce(IUIAutomationElement) -> Record,
    ) {
        let Some(sender) = sender else {
            return;
        };
        let key = kind.coalescing_key(&runtime_id(sender));
        self.shared.enqueue(key, make_record(sender.clone()));
    }
}

impl IUIAutomationEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleAutomationEvent(
        &self,
        sender: Option<&IUIAutomationElement>,
        eventid: UIA_EVENT_ID,
    ) -> Result<()> {
        self.enqueue(
            sender,
            EventKind::Automation {
                event_id: eventid.0,
            },
            |sender| Record::Automation {
                sender,
                event_id: eventid,
            },
        );
        Ok(())
    }
}

impl IUIAutomationFocusChangedEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        sender: Option<&IUIAutomationElement>,
    ) -> Result<()> {
        self.enqueue(sender, EventKind::FocusChanged, |sender| {
            Record::FocusChanged { sender }
        });
        Ok(())
    }
}

impl IUIAutomationPropertyChangedEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        sender: Option<&IUIAutomationElement>,
        propertyid: UIA_PROPERTY_ID,
        newvalue: &VARIANT,
    ) -> Result<()> {
        self.enqueue(
            sender,
            EventKind::PropertyChanged {
                property_id: propertyid.0,
            },
            |sender| Record::PropertyChanged {
                sender,
                property_id: propertyid,
                new_value: newvalue.clone(),
            },
        );
        Ok(())
    }
}

impl IUIAutomationNotificationEventHandler_Impl for RateLimitedEventHandler_Impl {
    fn HandleNotificationEvent(
        &self,
        sender: Option<&IUIAutomationElement>,
        notificationkind: NotificationKind,
        notificationprocessing: NotificationProcessing,
        displaystring: &BSTR,
        activityid: &BSTR,
    ) -> Result<()> {
        // A NULL activity-ID BSTR keys as `None` (a single 0 in the key);
        // a non-NULL one contributes its UTF-16 units.
        let activity_id = if activityid.is_empty() {
            // BSTR::is_empty() is true for NULL and for L"". The C++ keys
            // NULL as [0] and L"" as no chars; an empty BSTR here is treated
            // as NULL (the common case), which is the [0] branch.
            None
        } else {
            Some(activityid.as_wide().to_vec())
        };
        self.enqueue(
            sender,
            EventKind::Notification {
                kind: notificationkind.0,
                processing: notificationprocessing.0,
                activity_id,
            },
            |sender| Record::Notification {
                sender,
                kind: notificationkind,
                processing: notificationprocessing,
                display_string: displaystring.clone(),
                activity_id: activityid.clone(),
            },
        );
        Ok(())
    }
}

impl IUIAutomationActiveTextPositionChangedEventHandler_Impl
    for RateLimitedEventHandler_Impl
{
    fn HandleActiveTextPositionChangedEvent(
        &self,
        sender: Option<&IUIAutomationElement>,
        range: Option<&IUIAutomationTextRange>,
    ) -> Result<()> {
        // The range is nullable (as the C++ stores a possibly-NULL CComPtr);
        // store it as-is and forward it verbatim on flush.
        let range = range.cloned();
        self.enqueue(
            sender,
            EventKind::ActiveTextPositionChanged,
            move |sender| Record::ActiveTextPositionChanged { sender, range },
        );
        Ok(())
    }
}

/// Per-handler termination state, keyed by the COM object's raw pointer.
/// Mirrors the C++ `activeRateLimitedEventHandlers` set; here it also owns
/// the flusher `JoinHandle` so `terminate` can stop + join it.
struct Active {
    shared: Arc<Shared>,
    joiner: JoinHandle<()>,
}

fn registry() -> &'static Mutex<HashMap<usize, Active>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Active>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Create a rate-limited handler wrapping `existing`, start its flusher
/// thread, and return the new COM object via `out`. Port of
/// `rateLimitedUIAEventHandler_create` (api.cpp).
///
/// # Safety
///
/// `existing` must be a valid `IUnknown*` (the handler to forward to) and
/// `out` a valid `void**`. The returned handle is an owned COM reference the
/// caller must eventually `Release`.
#[no_mangle]
pub unsafe extern "C" fn rateLimitedUIAEventHandler_create(
    existing: *mut c_void,
    out: *mut *mut c_void,
) -> windows::core::HRESULT {
    if existing.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let existing = match unsafe { IUnknown::from_raw_borrowed(&existing) } {
        Some(u) => u,
        None => return E_INVALIDARG,
    };
    let handlers = ExistingHandlers::from_unknown(existing);
    let shared = Arc::new(Shared::new());
    let flusher_shared = shared.clone();
    let joiner = std::thread::spawn(move || {
        run_flusher(flusher_shared, handlers);
    });

    let handler = RateLimitedEventHandler {
        shared: shared.clone(),
    };
    // Hand out the object as IUnknown (COM identity); the caller (and UIA)
    // QI it for the specific handler interfaces.
    let unknown: IUnknown = handler.into();
    let raw = unknown.into_raw();
    registry()
        .lock()
        .unwrap()
        .insert(raw as usize, Active { shared, joiner });
    unsafe { *out = raw };
    S_OK
}

/// Stop and join a handler's flusher thread. Port of
/// `rateLimitedUIAEventHandler_terminate` (api.cpp): validates the handle,
/// signals the flusher to stop, and **blocks** until it has joined. Does not
/// release the COM object — that happens via COM refcounting once UIA and
/// Python drop their references.
///
/// # Safety
///
/// `handle` must be a handle previously returned by
/// [`rateLimitedUIAEventHandler_create`] and not yet terminated.
#[no_mangle]
pub unsafe extern "C" fn rateLimitedUIAEventHandler_terminate(
    handle: *mut c_void,
) -> windows::core::HRESULT {
    let active = match registry().lock().unwrap().remove(&(handle as usize)) {
        Some(a) => a,
        None => return E_INVALIDARG,
    };
    active.shared.request_stop();
    let _ = active.joiner.join();
    S_OK
}
