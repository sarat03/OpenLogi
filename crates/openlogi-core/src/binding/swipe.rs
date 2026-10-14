//! The swipe-gesture runtime machinery: travel thresholds, angle snapping, and
//! the [`SwipeAccumulator`] state machine shared by both gesture-capture paths.
//! This is input processing, distinct from the `Action` vocabulary the parent
//! [`binding`](super) module defines.
//!
//! The model is Logitech Options+'s own button-gesture recognizer, recovered
//! from `logioptionsplus_agent` (`logi::device_gestures::GestureTriggerBaseState`
//! and `ButtonGesture4wayState`) and confirmed against captured runtime
//! behaviour:
//!
//! - every raw-XY report is angle-snapped first: a report whose dominant axis
//!   is at least twice its cross axis keeps only the dominant axis, anything
//!   more diagonal (roughly 26.6°–63.4°) is discarded outright;
//! - snapped travel sums until the dominant axis reaches
//!   [`GESTURE_START_THRESHOLD`], which starts the gesture and locks its axis
//!   for the rest of the hold;
//! - travel along the locked axis then fires the direction every
//!   [`GESTURE_SWIPE_THRESHOLD`], no faster than [`GESTURE_ACTION_PAUSE`];
//!   backtracking past the start threshold turns the gesture around;
//! - a hold that never starts a gesture is a click.

use std::time::{Duration, Instant};

use super::GestureDirection;

/// Dominant-axis travel (raw-XY units) that starts a held gesture and locks its
/// axis. Options+'s `ButtonGesture4wayState::StartThreshold` is
/// `(1 - speed_factor) × 50`; the profiles it ships leave `speed_factor` at 0.
pub const GESTURE_START_THRESHOLD: i32 = 50;
/// Travel along the locked axis that fires the direction — Options+'s
/// `ActionThreshold`, `(1 - speed_factor) × 250`. A fired gesture keeps firing
/// every further `GESTURE_SWIPE_THRESHOLD` of travel the same way.
// ponytail: speed_factor fixed at Options+'s shipped 0; expose it as a
// sensitivity setting when someone asks to tune it.
pub const GESTURE_SWIPE_THRESHOLD: i32 = 250;
/// Minimum interval between two fires within one hold, so a fast continuous
/// motion steps through desktops at a readable pace instead of all at once
/// (Options+'s `pauseBetweenActionsMs` for its window-navigation preset).
pub const GESTURE_ACTION_PAUSE: Duration = Duration::from_millis(300);

/// Options+'s `GestureTriggerBaseState::AngleSnapping`: keep only the dominant
/// axis of a report that is at least twice as long as its cross axis, and drop
/// a more diagonal report entirely (`None`). Compared in `i64`, so the extreme
/// values a glitching device can report never panic or mis-snap.
#[must_use]
pub fn snap_angle(dx: i32, dy: i32) -> Option<(i32, i32)> {
    let (ax, ay) = (i64::from(dx).abs(), i64::from(dy).abs());
    if ax >= 2 * ay && ax > 0 {
        Some((dx, 0))
    } else if ay >= 2 * ax && ay > 0 {
        Some((0, dy))
    } else {
        None
    }
}

/// The started part of a hold: which way it is heading and how far it has
/// travelled toward the next fire.
#[derive(Debug, Clone, Copy)]
struct Stroke {
    direction: GestureDirection,
    /// Signed travel along `direction` since the last fire (or the start).
    along: i32,
}

/// The mid-swipe state machine shared by both gesture-capture paths: the HID++
/// gesture sources (`openlogi-hid`'s `0x1b04` raw-XY divert) and the OS-hook
/// Back/Forward buttons (`openlogi-agent-core`'s CGEventTap). See the module
/// docs for the recognizer it implements.
///
/// The two paths differ only in *what identifies the held control* (a
/// [`ButtonId`](super::ButtonId) for the OS hook, a diverted CID for the HID++
/// gesture control), so each owns that and embeds this for the shared travel
/// logic.
#[derive(Debug, Default)]
pub struct SwipeAccumulator {
    /// Whether a hold is in progress.
    holding: bool,
    /// Snapped travel summed before the gesture starts.
    pending: (i32, i32),
    /// The started gesture, once travel passed [`GESTURE_START_THRESHOLD`].
    stroke: Option<Stroke>,
    /// The snapped report most recently applied to the started gesture.
    motion: Option<(i32, i32)>,
    /// When the hold last fired, for [`GESTURE_ACTION_PAUSE`].
    fired_at: Option<Instant>,
}

impl SwipeAccumulator {
    /// Begin a fresh hold, discarding everything from any previous one.
    pub fn begin(&mut self) {
        *self = Self {
            holding: true,
            ..Self::default()
        };
    }

    /// Whether a hold is in progress (between [`Self::begin`] and [`Self::end`]),
    /// so callers can do rising/falling-edge detection without a second flag.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.holding
    }

    /// The started gesture's current direction, or `None` before it starts.
    /// Its axis is locked for the rest of the hold; only the sign can turn.
    #[must_use]
    pub fn direction(&self) -> Option<GestureDirection> {
        self.stroke.map(|stroke| stroke.direction)
    }

    /// The angle-snapped `(dx, dy)` of the last report the started gesture
    /// consumed — the per-report motion a continuous action (zoom, rotate)
    /// follows. `None` before the gesture starts or after a dropped report.
    #[must_use]
    pub fn motion(&self) -> Option<(i32, i32)> {
        self.motion
    }

    /// Feed one raw-XY delta into the current hold. Returns `Some(direction)`
    /// each time travel fires that direction; `None` while still short of the
    /// next fire, for a discarded diagonal report, or when not holding.
    pub fn accumulate(&mut self, dx: i32, dy: i32) -> Option<GestureDirection> {
        self.motion = None;
        if !self.holding {
            return None;
        }
        let (dx, dy) = snap_angle(dx, dy)?;
        let Some(stroke) = &mut self.stroke else {
            self.pending.0 = self.pending.0.saturating_add(dx);
            self.pending.1 = self.pending.1.saturating_add(dy);
            let (px, py) = self.pending;
            let direction = start_direction(px, py)?;
            let along = along(direction, px, py);
            self.stroke = Some(Stroke { direction, along });
            self.motion = Some((dx, dy));
            return self.fire_if_due(Instant::now());
        };
        // The axis is locked: cross-axis motion is ignored.
        let step = along(stroke.direction, dx, dy);
        if step == 0 {
            return None;
        }
        self.motion = Some((dx, dy));
        stroke.along = stroke.along.saturating_add(step);
        if stroke.along <= -GESTURE_START_THRESHOLD {
            // Backtracked past the start threshold: the gesture turns around
            // and counts from the turning point.
            stroke.direction = opposite(stroke.direction);
            stroke.along = stroke.along.saturating_neg() - GESTURE_START_THRESHOLD;
        }
        self.fire_if_due(Instant::now())
    }

    fn fire_if_due(&mut self, now: Instant) -> Option<GestureDirection> {
        let stroke = self.stroke.as_mut()?;
        if stroke.along < GESTURE_SWIPE_THRESHOLD {
            return None;
        }
        if self
            .fired_at
            .is_some_and(|at| now.saturating_duration_since(at) < GESTURE_ACTION_PAUSE)
        {
            // Too soon after the last fire: hold at the threshold rather than
            // banking travel, so the pause cannot release a burst.
            stroke.along = GESTURE_SWIPE_THRESHOLD;
            return None;
        }
        stroke.along -= GESTURE_SWIPE_THRESHOLD;
        self.fired_at = Some(now);
        Some(stroke.direction)
    }

    /// End the current hold. Returns `true` when an in-progress hold ended
    /// without ever starting a gesture — the caller should fire the plain
    /// `Click` action — and `false` when it started one (fired or not), or
    /// when there was no hold to end (a stray release reports no click).
    pub fn end(&mut self) -> bool {
        let was_click = self.holding && self.stroke.is_none();
        self.holding = false;
        was_click
    }

    /// Test-only seam: age the last fire past [`GESTURE_ACTION_PAUSE`], as if
    /// the pause had elapsed, without sleeping. A no-op before any fire.
    #[doc(hidden)]
    pub fn pause_for_test(&mut self) {
        if let Some(at) = self.fired_at {
            self.fired_at = at.checked_sub(GESTURE_ACTION_PAUSE * 2);
        }
    }
}

/// The direction summed pre-start travel commits to once its dominant axis
/// reaches [`GESTURE_START_THRESHOLD`]. Coordinates follow the device's raw-XY
/// convention (`+x` = right, `+y` = down).
fn start_direction(dx: i32, dy: i32) -> Option<GestureDirection> {
    let (ax, ay) = (dx.saturating_abs(), dy.saturating_abs());
    if ax.max(ay) < GESTURE_START_THRESHOLD {
        return None;
    }
    Some(if ax >= ay {
        if dx > 0 {
            GestureDirection::Right
        } else {
            GestureDirection::Left
        }
    } else if dy > 0 {
        GestureDirection::Down
    } else {
        GestureDirection::Up
    })
}

/// The signed component of a report along `direction` (positive continues it).
fn along(direction: GestureDirection, dx: i32, dy: i32) -> i32 {
    match direction {
        GestureDirection::Left => dx.saturating_neg(),
        GestureDirection::Right => dx,
        GestureDirection::Up => dy.saturating_neg(),
        GestureDirection::Down => dy,
        GestureDirection::Click => 0,
    }
}

fn opposite(direction: GestureDirection) -> GestureDirection {
    match direction {
        GestureDirection::Left => GestureDirection::Right,
        GestureDirection::Right => GestureDirection::Left,
        GestureDirection::Up => GestureDirection::Down,
        GestureDirection::Down => GestureDirection::Up,
        GestureDirection::Click => GestureDirection::Click,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRE: i32 = GESTURE_SWIPE_THRESHOLD;

    fn held() -> SwipeAccumulator {
        let mut acc = SwipeAccumulator::default();
        acc.begin();
        acc
    }

    #[test]
    fn snap_angle_keeps_the_dominant_axis_at_two_to_one() {
        assert_eq!(snap_angle(20, 10), Some((20, 0)));
        assert_eq!(snap_angle(-10, 20), Some((0, 20)));
        assert_eq!(snap_angle(7, 0), Some((7, 0)));
        // Just inside 2:1 is too diagonal on both axes and is dropped.
        assert_eq!(snap_angle(19, 10), None);
        assert_eq!(snap_angle(10, 10), None);
        assert_eq!(snap_angle(0, 0), None);
    }

    #[test]
    fn snap_angle_does_not_panic_on_extreme_values() {
        assert_eq!(snap_angle(i32::MIN, 0), Some((i32::MIN, 0)));
        assert_eq!(snap_angle(0, i32::MAX), Some((0, i32::MAX)));
        assert_eq!(snap_angle(i32::MIN, i32::MIN), None);
    }

    #[test]
    fn a_hold_starts_at_fifty_and_fires_at_two_fifty() {
        let mut acc = held();
        assert_eq!(acc.accumulate(GESTURE_START_THRESHOLD - 1, 0), None);
        assert_eq!(
            acc.direction(),
            None,
            "not started below the start threshold"
        );
        assert_eq!(acc.accumulate(1, 0), None);
        assert_eq!(acc.direction(), Some(GestureDirection::Right));
        assert_eq!(acc.accumulate(FIRE - GESTURE_START_THRESHOLD - 1, 0), None);
        assert_eq!(acc.accumulate(1, 0), Some(GestureDirection::Right));
    }

    #[test]
    fn one_report_can_start_and_fire() {
        let mut acc = held();
        assert_eq!(acc.accumulate(0, -(FIRE + 10)), Some(GestureDirection::Up));
    }

    #[test]
    fn diagonal_reports_are_discarded_not_summed() {
        let mut acc = held();
        for _ in 0..100 {
            assert_eq!(acc.accumulate(30, 30), None);
        }
        assert_eq!(acc.direction(), None);
        assert!(acc.end(), "a hold of pure diagonals is a click");
    }

    #[test]
    fn the_axis_locks_once_started() {
        let mut acc = held();
        assert_eq!(acc.accumulate(60, 0), None);
        // A long vertical stroke after the lock never fires Up or Down.
        for _ in 0..50 {
            assert_eq!(acc.accumulate(0, -40), None);
        }
        assert_eq!(acc.direction(), Some(GestureDirection::Right));
        assert_eq!(acc.accumulate(FIRE, 0), Some(GestureDirection::Right));
    }

    #[test]
    fn continuing_fires_again_after_the_pause() {
        let mut acc = held();
        assert_eq!(acc.accumulate(FIRE, 0), Some(GestureDirection::Right));
        // Within the pause the next threshold is held, not banked.
        assert_eq!(acc.accumulate(3 * FIRE, 0), None);
        acc.pause_for_test();
        assert_eq!(acc.accumulate(1, 0), Some(GestureDirection::Right));
        acc.pause_for_test();
        assert_eq!(
            acc.accumulate(1, 0),
            None,
            "the pause clamped travel instead of banking a burst"
        );
    }

    #[test]
    fn backtracking_past_the_start_threshold_turns_around() {
        let mut acc = held();
        assert_eq!(acc.accumulate(-FIRE, 0), Some(GestureDirection::Left));
        acc.pause_for_test();
        // Back right: the first START of travel only turns the gesture.
        assert_eq!(acc.accumulate(GESTURE_START_THRESHOLD, 0), None);
        assert_eq!(acc.direction(), Some(GestureDirection::Right));
        assert_eq!(acc.accumulate(FIRE - 1, 0), None);
        assert_eq!(acc.accumulate(1, 0), Some(GestureDirection::Right));
    }

    #[test]
    fn a_wobble_shorter_than_the_start_threshold_never_turns() {
        let mut acc = held();
        assert_eq!(acc.accumulate(FIRE, 0), Some(GestureDirection::Right));
        for _ in 0..20 {
            assert_eq!(acc.accumulate(-20, 0), None);
            assert_eq!(acc.accumulate(20, 0), None);
        }
        assert_eq!(acc.direction(), Some(GestureDirection::Right));
    }

    #[test]
    fn motion_reports_the_snapped_delta_after_start_only() {
        let mut acc = held();
        acc.accumulate(0, 10);
        assert_eq!(acc.motion(), None, "not started yet");
        acc.accumulate(3, 60);
        assert_eq!(acc.motion(), Some((0, 60)), "snapped to the dominant axis");
        acc.accumulate(25, 25);
        assert_eq!(acc.motion(), None, "a dropped diagonal carries no motion");
        acc.accumulate(40, 0);
        assert_eq!(acc.motion(), None, "cross-axis motion after the lock");
    }

    #[test]
    fn end_reports_click_only_when_no_gesture_started() {
        let mut acc = held();
        assert_eq!(acc.accumulate(2, -1), None);
        assert!(acc.end(), "a hold that never started is a click");

        acc.begin();
        acc.accumulate(GESTURE_START_THRESHOLD, 0);
        assert!(!acc.end(), "a started gesture is not a click, even unfired");
    }

    #[test]
    fn motion_outside_a_hold_is_ignored() {
        let mut acc = SwipeAccumulator::default();
        assert!(!acc.is_holding());
        assert_eq!(acc.accumulate(FIRE * 2, 0), None);
    }

    #[test]
    fn saturating_travel_never_panics() {
        let mut acc = held();
        assert_eq!(acc.accumulate(i32::MAX, 0), Some(GestureDirection::Right));
        acc.pause_for_test();
        assert_eq!(acc.accumulate(i32::MAX, 0), Some(GestureDirection::Right));
        assert_eq!(acc.accumulate(i32::MIN, 0), None);
    }

    #[test]
    fn begin_recovers_a_stale_hold() {
        let mut acc = held();
        assert_eq!(acc.accumulate(-FIRE, 0), Some(GestureDirection::Left));
        // No end(): a dropped release, then a fresh press.
        acc.begin();
        assert_eq!(acc.direction(), None);
        assert_eq!(acc.accumulate(FIRE, 0), Some(GestureDirection::Right));
    }

    #[test]
    fn end_without_a_hold_is_not_a_click() {
        let mut acc = SwipeAccumulator::default();
        assert!(!acc.end());
        acc.begin();
        assert!(acc.end());
        assert!(!acc.end(), "a redundant second release is inert");
    }
}
