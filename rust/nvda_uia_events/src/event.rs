//! Coalescing keys for UIA events.
//!
//! Port of the `generateCoalescingKey()` methods on the event-record
//! structs in `nvdaHelper/local/UIAEventLimiter/eventRecord.h`. The C++
//! uses a `std::variant` of five record types; here that sum type is an
//! `enum`. This module holds only the *discriminating* data needed to build
//! the coalescing key — pure, no COM. Phase 2 pairs an `EventKind` with the
//! COM element / range / VARIANT that actually gets emitted.

use windows::Win32::UI::Accessibility::{
    UIA_ActiveTextPositionChangedEventId, UIA_AutomationFocusChangedEventId,
    UIA_AutomationPropertyChangedEventId, UIA_NotificationEventId,
};

/// The kind of a queued UIA event, carrying the per-kind discriminators
/// that (together with the sender element's RuntimeId) form its coalescing
/// key. Two queued events with equal keys are duplicates; the newer wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A plain automation event, keyed by its event ID.
    Automation { event_id: i32 },
    /// A focus change (no extra discriminator).
    FocusChanged,
    /// A property change, keyed by the property ID.
    PropertyChanged { property_id: i32 },
    /// A notification, keyed by kind + processing + the activity ID.
    Notification {
        kind: i32,
        processing: i32,
        /// The activity-ID string as UTF-16, or `None` for a NULL BSTR.
        activity_id: Option<Vec<u16>>,
    },
    /// An active-text-position change (no extra discriminator).
    ActiveTextPositionChanged,
}

impl EventKind {
    /// Build the coalescing key: the sender's `runtime_id` followed by the
    /// per-kind discriminators. Faithful to the C++
    /// `generateCoalescingKey()`, including the notification rule of
    /// appending each activity-ID UTF-16 unit as an `int`, or a single `0`
    /// for a NULL activity ID.
    pub fn coalescing_key(&self, runtime_id: &[i32]) -> Vec<i32> {
        let mut key = runtime_id.to_vec();
        match self {
            EventKind::Automation { event_id } => key.push(*event_id),
            EventKind::FocusChanged => {
                key.push(UIA_AutomationFocusChangedEventId.0)
            }
            EventKind::PropertyChanged { property_id } => {
                key.push(UIA_AutomationPropertyChangedEventId.0);
                key.push(*property_id);
            }
            EventKind::Notification {
                kind,
                processing,
                activity_id,
            } => {
                key.push(UIA_NotificationEventId.0);
                key.push(*kind);
                key.push(*processing);
                match activity_id {
                    Some(chars) => key.extend(chars.iter().map(|&c| c as i32)),
                    None => key.push(0),
                }
            }
            EventKind::ActiveTextPositionChanged => {
                key.push(UIA_ActiveTextPositionChangedEventId.0)
            }
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RID: [i32; 3] = [42, 7, 99];

    #[test]
    fn automation_key_is_runtime_id_plus_event_id() {
        let k = EventKind::Automation { event_id: 20005 }.coalescing_key(&RID);
        assert_eq!(k, vec![42, 7, 99, 20005]);
    }

    #[test]
    fn focus_key_appends_the_focus_event_id() {
        let k = EventKind::FocusChanged.coalescing_key(&RID);
        assert_eq!(k, vec![42, 7, 99, UIA_AutomationFocusChangedEventId.0]);
    }

    #[test]
    fn property_key_appends_event_then_property_id() {
        let k = EventKind::PropertyChanged { property_id: 30003 }
            .coalescing_key(&RID);
        assert_eq!(
            k,
            vec![42, 7, 99, UIA_AutomationPropertyChangedEventId.0, 30003]
        );
    }

    #[test]
    fn notification_key_includes_kind_processing_and_activity_chars() {
        let k = EventKind::Notification {
            kind: 2,
            processing: 1,
            activity_id: Some("ab".encode_utf16().collect()),
        }
        .coalescing_key(&RID);
        assert_eq!(
            k,
            vec![
                42,
                7,
                99,
                UIA_NotificationEventId.0,
                2,
                1,
                'a' as i32,
                'b' as i32,
            ]
        );
    }

    #[test]
    fn notification_null_activity_id_appends_a_single_zero() {
        let k = EventKind::Notification {
            kind: 2,
            processing: 1,
            activity_id: None,
        }
        .coalescing_key(&RID);
        assert_eq!(k, vec![42, 7, 99, UIA_NotificationEventId.0, 2, 1, 0]);
    }

    #[test]
    fn notification_empty_activity_id_appends_no_chars() {
        // A non-NULL but empty activity ID contributes no characters (and no
        // trailing zero) — matching the C++ `for(c: view) push(c)` on an
        // empty view.
        let k = EventKind::Notification {
            kind: 2,
            processing: 1,
            activity_id: Some(Vec::new()),
        }
        .coalescing_key(&RID);
        assert_eq!(k, vec![42, 7, 99, UIA_NotificationEventId.0, 2, 1]);
    }

    #[test]
    fn active_text_position_key_appends_its_event_id() {
        let k = EventKind::ActiveTextPositionChanged.coalescing_key(&RID);
        assert_eq!(
            k,
            vec![42, 7, 99, UIA_ActiveTextPositionChangedEventId.0]
        );
    }

    #[test]
    fn different_property_ids_do_not_collide() {
        let a = EventKind::PropertyChanged { property_id: 1 }.coalescing_key(&RID);
        let b = EventKind::PropertyChanged { property_id: 2 }.coalescing_key(&RID);
        assert_ne!(a, b);
    }

    #[test]
    fn same_element_different_kinds_do_not_collide() {
        let f = EventKind::FocusChanged.coalescing_key(&RID);
        let a = EventKind::ActiveTextPositionChanged.coalescing_key(&RID);
        assert_ne!(f, a);
    }
}
