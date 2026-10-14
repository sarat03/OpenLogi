use openlogi_hid::thumbwheel::WheelResolution;

use super::wheel::{ScrollScale, WheelOutput, WheelRotation};
use super::*;

fn rotation(magnitude: i32) -> WheelRotation {
    let increments = i16::try_from(magnitude).expect("test magnitude fits in i16");
    WheelRotation::from_increments(increments).expect("non-zero test rotation")
}

fn scale() -> ScrollScale {
    ScrollScale::new(WheelResolution::UNKNOWN, ThumbwheelSensitivity::DEFAULT)
}

#[test]
fn replacement_session_does_not_inherit_progress_or_cooldown() {
    let old = HidppSessionId::with_epoch("mouse-a", 7);
    let replacement = HidppSessionId::with_epoch("mouse-a", 8);
    let threshold = ThumbwheelSensitivity::DEFAULT.action_threshold();
    let now = Instant::now();
    let mut wheels = SessionWheels::default();

    assert_eq!(
        wheels
            .for_session(&old)
            .advance(rotation(threshold), &Action::VolumeUp, scale(), now,),
        WheelOutput::FireAction
    );
    assert_eq!(
        wheels.for_session(&replacement).advance(
            rotation(threshold),
            &Action::VolumeUp,
            scale(),
            now,
        ),
        WheelOutput::FireAction,
        "a new session must not inherit the old session's cooldown"
    );

    wheels.cancel_session(&old);
    assert!(
        wheels.0.contains_key(&replacement),
        "canceling a stale epoch must not erase its replacement's state"
    );
}

#[test]
fn replacement_session_does_not_inherit_partial_progress() {
    let old = HidppSessionId::with_epoch("mouse-a", 7);
    let replacement = HidppSessionId::with_epoch("mouse-a", 8);
    let threshold = ThumbwheelSensitivity::DEFAULT.action_threshold();
    let now = Instant::now();
    let mut wheels = SessionWheels::default();

    assert_eq!(
        wheels
            .for_session(&old)
            .advance(rotation(threshold - 1), &Action::VolumeUp, scale(), now,),
        WheelOutput::Idle
    );
    assert_eq!(
        wheels
            .for_session(&replacement)
            .advance(rotation(1), &Action::VolumeUp, scale(), now,),
        WheelOutput::Idle,
        "a new session must start with no action progress"
    );
}

#[test]
fn a_desktop_binding_streams_a_dock_swipe_signed_by_its_direction() {
    let mut wheels = SessionWheels::default();
    let session = HidppSessionId::with_epoch("mouse-a", 7);
    let now = Instant::now();
    let WheelOutput::DesktopSwipe(next) =
        wheels
            .for_session(&session)
            .advance(rotation(3), &Action::NextDesktop, scale(), now)
    else {
        panic!("a desktop binding streams a swipe");
    };
    let WheelOutput::DesktopSwipe(previous) =
        wheels
            .for_session(&session)
            .advance(rotation(-3), &Action::PreviousDesktop, scale(), now)
    else {
        panic!("a desktop binding streams a swipe");
    };
    assert!(next > 0.0 && previous < 0.0, "{next} {previous}");
    assert!(
        (next + previous).abs() < f64::EPSILON,
        "equal rolls, equal progress"
    );
}

#[test]
fn continuous_gestures_follow_the_locked_axis() {
    let zoom_in = continuous_gesture(&Action::GestureZoom, GestureDirection::Up, 0, -10);
    assert!(matches!(zoom_in, Some((ContinuousGesture::Zoom, d)) if d > 0.0));
    let zoom_out = continuous_gesture(&Action::GestureZoom, GestureDirection::Down, 0, 10);
    assert!(matches!(zoom_out, Some((ContinuousGesture::Zoom, d)) if d < 0.0));
    let clockwise = continuous_gesture(&Action::GestureRotate, GestureDirection::Right, 10, 0);
    assert!(matches!(clockwise, Some((ContinuousGesture::Rotate, d)) if d < 0.0));
    assert_eq!(
        continuous_gesture(&Action::Copy, GestureDirection::Up, 0, -10),
        None
    );
    assert_eq!(
        continuous_gesture(&Action::GestureZoom, GestureDirection::Click, 0, 0),
        None
    );
}
