//! Private CGEvent encodings of trackpad gestures, field for field as Logi
//! Options+ posts them (captured from `logioptionsplus_agent` with a passive
//! event tap): Dock swipes as `kCGSEventDockControl` (type 30), magnify,
//! rotate and smart zoom as `kCGSEventGesture` (type 29).

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGEvent, CGEventField, CGEventTapLocation, CGEventType};

use super::super::gesture::{ContinuousGesture, GestureFrame, GesturePhase};

/// The private CGEvent types.
const EVENT_GESTURE: u32 = 29;
const EVENT_DOCK_CONTROL: u32 = 30;

/// Private fields, numbered as WindowServer reads them.
const FIELD_HID_TYPE: u32 = 110;
const FIELD_MAGNIFICATION: u32 = 113;
const FIELD_ROTATION: u32 = 114;
const FIELD_SWIPE_DIRECTION: u32 = 115;
const FIELD_SWIPE_MOTION: u32 = 123;
const FIELD_SWIPE_PROGRESS: u32 = 124;
const FIELD_SWIPE_VELOCITY: u32 = 129;
const FIELD_PHASE: u32 = 132;
const FIELD_DOCK_PHASE: u32 = 134;
const FIELD_DOCK_CONSTANT: u32 = 138;

/// `IOHIDEventType` values carried in [`FIELD_HID_TYPE`].
const HID_ROTATION: i64 = 5;
const HID_ZOOM: i64 = 8;
const HID_ZOOM_TOGGLE: i64 = 22;
const HID_DOCK_SWIPE: i64 = 23;

/// `IOHIDSwipeMask` bits: the Dock swipe's direction, set on its first change.
const SWIPE_LEFT: i64 = 4;
const SWIPE_RIGHT: i64 = 8;

/// Post one frame of a streamed gesture.
pub(in crate::inject) fn post_gesture(frame: &GestureFrame) {
    let Some(event) = gesture_event(frame) else {
        tracing::warn!(?frame, "gesture event allocation failed");
        return;
    };
    CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
}

/// Post a one-shot smart zoom (the trackpad's two-finger double tap).
pub(super) fn smart_zoom() {
    let Some(event) = new_event(EVENT_GESTURE) else {
        tracing::warn!("smart-zoom event allocation failed");
        return;
    };
    set_int(&event, FIELD_HID_TYPE, HID_ZOOM_TOGGLE);
    set_int(&event, FIELD_PHASE, 0);
    CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
}

fn gesture_event(frame: &GestureFrame) -> Option<CFRetained<CGEvent>> {
    let phase = match frame.phase {
        GesturePhase::Began => 1,
        GesturePhase::Changed => 2,
        GesturePhase::Ended => 4,
    };
    let event = match frame.kind {
        ContinuousGesture::DesktopSwipe => {
            let event = new_event(EVENT_DOCK_CONTROL)?;
            set_int(&event, FIELD_HID_TYPE, HID_DOCK_SWIPE);
            set_int(&event, FIELD_SWIPE_MOTION, 1); // horizontal
            set_int(&event, FIELD_DOCK_PHASE, phase);
            set_int(&event, FIELD_DOCK_CONSTANT, 3);
            set_double(&event, FIELD_SWIPE_PROGRESS, frame.total);
            if frame.first_change {
                let mask = if frame.total < 0.0 {
                    SWIPE_LEFT
                } else {
                    SWIPE_RIGHT
                };
                set_int(&event, FIELD_SWIPE_DIRECTION, mask);
            }
            if frame.phase == GesturePhase::Ended {
                set_double(&event, FIELD_SWIPE_VELOCITY, frame.velocity);
            }
            event
        }
        ContinuousGesture::Zoom => {
            let event = new_event(EVENT_GESTURE)?;
            set_int(&event, FIELD_HID_TYPE, HID_ZOOM);
            set_double(&event, FIELD_MAGNIFICATION, frame.delta);
            event
        }
        ContinuousGesture::Rotate => {
            let event = new_event(EVENT_GESTURE)?;
            set_int(&event, FIELD_HID_TYPE, HID_ROTATION);
            set_double(&event, FIELD_ROTATION, frame.delta);
            event
        }
    };
    set_int(&event, FIELD_PHASE, phase);
    Some(event)
}

/// A cursor-located event of private `event_type`, tagged as OpenLogi's own.
fn new_event(event_type: u32) -> Option<CFRetained<CGEvent>> {
    let event = CGEvent::new(None)?;
    // Establish the private type before setting its fields, as
    // CGEventSetIntegerValueField's contract requires.
    CGEvent::set_type(Some(&event), CGEventType(event_type));
    CGEvent::set_integer_value_field(
        Some(&event),
        CGEventField::EventSourceUserData,
        super::super::SYNTHETIC_EVENT_USER_DATA,
    );
    Some(event)
}

fn set_int(event: &CGEvent, field: u32, value: i64) {
    CGEvent::set_integer_value_field(Some(event), CGEventField(field), value);
}

fn set_double(event: &CGEvent, field: u32, value: f64) {
    CGEvent::set_double_value_field(Some(event), CGEventField(field), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: ContinuousGesture, phase: GesturePhase, total: f64) -> GestureFrame {
        GestureFrame {
            kind,
            phase,
            delta: total,
            total,
            velocity: -0.5,
            first_change: phase == GesturePhase::Changed,
        }
    }

    fn int(event: &CGEvent, field: u32) -> i64 {
        CGEvent::integer_value_field(Some(event), CGEventField(field))
    }

    fn double(event: &CGEvent, field: u32) -> f64 {
        CGEvent::double_value_field(Some(event), CGEventField(field))
    }

    #[test]
    fn dock_swipe_frames_match_the_captured_options_plus_layout() {
        let changed = gesture_event(&frame(
            ContinuousGesture::DesktopSwipe,
            GesturePhase::Changed,
            -0.25,
        ))
        .expect("event");
        assert_eq!(int(&changed, 55), i64::from(EVENT_DOCK_CONTROL));
        for (field, value) in [
            (FIELD_HID_TYPE, HID_DOCK_SWIPE),
            (FIELD_SWIPE_MOTION, 1),
            (FIELD_PHASE, 2),
            (FIELD_DOCK_PHASE, 2),
            (FIELD_DOCK_CONSTANT, 3),
            (FIELD_SWIPE_DIRECTION, SWIPE_LEFT),
        ] {
            assert_eq!(int(&changed, field), value, "field {field}");
        }
        assert!((double(&changed, FIELD_SWIPE_PROGRESS) + 0.25).abs() < 1e-6);

        let ended = gesture_event(&frame(
            ContinuousGesture::DesktopSwipe,
            GesturePhase::Ended,
            0.6,
        ))
        .expect("event");
        assert_eq!(int(&ended, FIELD_PHASE), 4);
        assert_eq!(
            int(&ended, FIELD_SWIPE_DIRECTION),
            0,
            "direction only on the first change"
        );
        assert!((double(&ended, FIELD_SWIPE_VELOCITY) + 0.5).abs() < 1e-6);
        assert_eq!(
            int(&ended, CGEventField::EventSourceUserData.0),
            super::super::super::SYNTHETIC_EVENT_USER_DATA
        );
    }

    #[test]
    fn zoom_and_rotate_carry_their_delta_in_their_own_field() {
        let zoom = gesture_event(&frame(ContinuousGesture::Zoom, GesturePhase::Began, 0.0))
            .expect("event");
        assert_eq!(int(&zoom, 55), i64::from(EVENT_GESTURE));
        assert_eq!(int(&zoom, FIELD_HID_TYPE), HID_ZOOM);
        assert_eq!(int(&zoom, FIELD_PHASE), 1);

        let rotate = gesture_event(&frame(
            ContinuousGesture::Rotate,
            GesturePhase::Changed,
            3.5,
        ))
        .expect("event");
        assert_eq!(int(&rotate, FIELD_HID_TYPE), HID_ROTATION);
        assert!((double(&rotate, FIELD_ROTATION) - 3.5).abs() < 1e-6);
    }
}
