//! OS input-event synthesis split out of openlogi-core so the core stays platform- and IO-free.

mod inject;

pub use inject::{
    ContinuousGesture, HeldChord, SYNTHETIC_EVENT_USER_DATA, SmoothScrollPhase,
    ax_navigate_browser, end_gesture, execute, post_scroll, post_smooth_scroll, press_hold,
    stream_gesture,
};

#[cfg(target_os = "linux")]
pub use inject::action_device_path;
#[cfg(target_os = "macos")]
pub use inject::prepare_menu_shortcuts;
