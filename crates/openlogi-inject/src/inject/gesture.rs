//! Continuous trackpad-style gestures — a Dock desktop swipe, a magnify, a
//! rotation — streamed from a held gesture button or a rolling thumb wheel.
//!
//! The event shapes are Logi Options+'s, captured at runtime from
//! `logioptionsplus_agent`: one began frame, a changed frame per input report,
//! and one ended frame. Callers only push deltas; this module owns the phase
//! bookkeeping and ends a gesture its source stopped feeding (a thumb wheel has
//! no release edge) after [`IDLE_END`].

use std::sync::LazyLock;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use super::platform;

/// How long a streamed gesture may go without a delta before it ends on its
/// own — about the gap between two separate thumb-wheel rolls.
const IDLE_END: Duration = Duration::from_millis(250);

/// One kind of continuous gesture OpenLogi synthesises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuousGesture {
    /// A horizontal Dock swipe between desktops (Spaces). Deltas are swipe
    /// progress in desktop widths: negative toward the previous desktop,
    /// positive toward the next.
    DesktopSwipe,
    /// A magnify (pinch) gesture. Deltas are magnification, positive zooms in.
    Zoom,
    /// A rotation gesture. Deltas are degrees, positive is counter-clockwise.
    Rotate,
}

/// Where one posted frame sits in its gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GesturePhase {
    Began,
    Changed,
    Ended,
}

/// One synthetic gesture event for the platform layer to post.
// Not `expect`: off macOS the lint fires in the `--lib` build but not the
// `--test` one, whose tests read every field.
#[cfg_attr(
    not(target_os = "macos"),
    expect(clippy::allow_attributes, reason = "see above"),
    allow(dead_code, reason = "only the macOS encoder reads every field")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GestureFrame {
    pub(super) kind: ContinuousGesture,
    pub(super) phase: GesturePhase,
    /// This frame's own delta (zero on began/ended).
    pub(super) delta: f64,
    /// The gesture's running total, including `delta`.
    pub(super) total: f64,
    /// Delta per second at the last change; only meaningful on ended.
    pub(super) velocity: f64,
    /// The gesture's first changed frame, which carries the swipe direction.
    pub(super) first_change: bool,
}

/// Stream `delta` into the current `kind` gesture, beginning one if none is
/// active (and ending a gesture of another kind first). Non-blocking: safe on
/// input-dispatch threads. Non-finite deltas are dropped at this boundary.
pub fn stream_gesture(kind: ContinuousGesture, delta: f64) {
    if delta.is_finite() {
        send(Command::Delta(kind, delta));
    }
}

/// End the streamed gesture now (its source was released). A no-op when none
/// is active.
pub fn end_gesture() {
    send(Command::End);
}

enum Command {
    Delta(ContinuousGesture, f64),
    End,
}

fn send(command: Command) {
    static WORKER: LazyLock<Option<Sender<Command>>> = LazyLock::new(spawn_worker);
    if let Some(worker) = WORKER.as_ref()
        && worker.send(command).is_err()
    {
        tracing::warn!("gesture worker gone — frame dropped");
    }
}

fn spawn_worker() -> Option<Sender<Command>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("openlogi-gesture".into())
        .spawn(move || {
            let mut stream = GestureStream::default();
            loop {
                let command = if stream.is_active() {
                    match rx.recv_timeout(IDLE_END) {
                        Ok(command) => Some(command),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    let Ok(command) = rx.recv() else { break };
                    Some(command)
                };
                let frames = match command {
                    Some(Command::Delta(kind, delta)) => stream.delta(kind, delta, Instant::now()),
                    Some(Command::End) | None => stream.end().into_iter().collect(),
                };
                for frame in &frames {
                    platform::post_gesture(frame);
                }
            }
        });
    match spawned {
        Ok(_) => Some(tx),
        Err(error) => {
            tracing::warn!(%error, "gesture worker failed to start");
            None
        }
    }
}

/// The sans-I/O phase bookkeeping behind the worker: which gesture is active
/// and the frames each input produces.
#[derive(Debug, Default)]
struct GestureStream {
    active: Option<Active>,
}

#[derive(Debug)]
struct Active {
    kind: ContinuousGesture,
    total: f64,
    velocity: f64,
    changed: bool,
    last_at: Instant,
}

impl GestureStream {
    fn is_active(&self) -> bool {
        self.active.is_some()
    }

    fn delta(&mut self, kind: ContinuousGesture, delta: f64, now: Instant) -> Vec<GestureFrame> {
        let mut frames = Vec::new();
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.kind != kind)
        {
            frames.extend(self.end());
        }
        let active = self.active.get_or_insert_with(|| {
            frames.push(GestureFrame {
                kind,
                phase: GesturePhase::Began,
                delta: 0.0,
                total: 0.0,
                velocity: 0.0,
                first_change: false,
            });
            Active {
                kind,
                total: 0.0,
                velocity: 0.0,
                changed: false,
                last_at: now,
            }
        });
        let seconds = now.saturating_duration_since(active.last_at).as_secs_f64();
        active.velocity = if seconds > 0.0 { delta / seconds } else { 0.0 };
        active.total += delta;
        active.last_at = now;
        frames.push(GestureFrame {
            kind,
            phase: GesturePhase::Changed,
            delta,
            total: active.total,
            velocity: active.velocity,
            first_change: !active.changed,
        });
        active.changed = true;
        frames
    }

    fn end(&mut self) -> Option<GestureFrame> {
        let active = self.active.take()?;
        Some(GestureFrame {
            kind: active.kind,
            phase: GesturePhase::Ended,
            delta: 0.0,
            total: active.total,
            velocity: active.velocity,
            first_change: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phases(frames: &[GestureFrame]) -> Vec<GesturePhase> {
        frames.iter().map(|frame| frame.phase).collect()
    }

    #[test]
    fn a_stream_is_began_changed_ended_with_a_running_total() {
        let mut stream = GestureStream::default();
        let t0 = Instant::now();
        let first = stream.delta(ContinuousGesture::Zoom, 0.25, t0);
        assert_eq!(phases(&first), [GesturePhase::Began, GesturePhase::Changed]);
        assert!(first[1].first_change);

        let second = stream.delta(
            ContinuousGesture::Zoom,
            0.5,
            t0 + Duration::from_millis(500),
        );
        assert_eq!(phases(&second), [GesturePhase::Changed]);
        assert!(!second[0].first_change, "only the first change is marked");
        assert!((second[0].total - 0.75).abs() < f64::EPSILON);
        assert!((second[0].velocity - 1.0).abs() < 1e-9, "0.5 per 500 ms");

        let end = stream.end().expect("an active gesture ends");
        assert_eq!(end.phase, GesturePhase::Ended);
        assert!((end.total - 0.75).abs() < f64::EPSILON);
        assert!(!stream.is_active());
        assert_eq!(stream.end(), None, "ending twice posts nothing");
    }

    #[test]
    fn switching_kind_ends_the_previous_gesture_first() {
        let mut stream = GestureStream::default();
        let now = Instant::now();
        stream.delta(ContinuousGesture::Zoom, 0.1, now);
        let frames = stream.delta(ContinuousGesture::Rotate, 5.0, now);
        assert_eq!(
            frames
                .iter()
                .map(|frame| (frame.kind, frame.phase))
                .collect::<Vec<_>>(),
            [
                (ContinuousGesture::Zoom, GesturePhase::Ended),
                (ContinuousGesture::Rotate, GesturePhase::Began),
                (ContinuousGesture::Rotate, GesturePhase::Changed),
            ]
        );
    }
}
