//! The custom shortcut field: a text input plus a press-to-record button.
//!
//! Clicking Record moves focus onto the recorder, and the next chord pressed is
//! written into the input as its canonical text, so the editor's Add button and
//! Tap/Hold choice commit it unchanged. Typing a shortcut stays available. The
//! recorder takes its own focus because the text input's
//! copy/paste/select-all bindings dispatch before any key listener and would
//! consume ⌘C, ⌘V and ⌘A. App-wide bindings (⌘Q, ⌘W, ⌘H, ⌘M, ⌘,) still win,
//! so those chords have to be typed.

use gpui::{
    Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke,
    ParentElement as _, Render, Styled as _, Window, div,
};
use gpui_component::input::InputState;
use gpui_component::{Selectable as _, h_flex};
use openlogi_core::binding::KeyCombo;

use crate::ui::components::{control_button, control_input};

/// A shortcut input that can also record the next pressed chord.
pub(crate) struct ShortcutRecorder {
    id: &'static str,
    focus: FocusHandle,
    input: Entity<InputState>,
}

impl ShortcutRecorder {
    pub(crate) fn new(id: &'static str, input: Entity<InputState>, cx: &mut Context<Self>) -> Self {
        Self {
            id,
            focus: cx.focus_handle(),
            input,
        }
    }

    /// The text input the shortcut is typed or recorded into.
    pub(crate) fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        // A bare Escape cancels; any other key that is not a chord (a lone
        // modifier, an unsupported key) keeps listening.
        if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
            window.blur(cx);
            cx.notify();
            return;
        }
        let Some(combo) = recorded_combo(&event.keystroke) else {
            return;
        };
        let text = combo.rendered_label();
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        window.blur(cx);
        cx.notify();
    }
}

impl Render for ShortcutRecorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = self.focus.is_focused(window);
        let focus = self.focus.clone();
        let id = self.id;
        let record_button = control_button(format!("{id}-custom-shortcut-record"))
            .debug_selector(move || format!("{id}-custom-shortcut-record"))
            .selected(recording)
            .label(if recording {
                tr!("actions.shortcut_recording")
            } else {
                tr!("actions.shortcut_record")
            })
            .on_click(move |_, window, cx| {
                window.focus(&focus, cx);
                cx.stop_propagation();
            });
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(move || format!("{id}-custom-shortcut-input"))
                    .child(control_input(&self.input).cleanable(true)),
            )
            // Only the button tracks recorder focus, so typing in the input
            // never reaches the recorder's key listener.
            .child(
                div()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .child(record_button),
            )
    }
}

/// The chord a key-down names, or `None` when it is not a supported shortcut
/// (a lone modifier, or a key the shortcut vocabulary lacks). Routing through
/// [`KeyCombo`]'s parser keeps one owner for which keys are valid.
fn recorded_combo(keystroke: &Keystroke) -> Option<KeyCombo> {
    let modifiers = &keystroke.modifiers;
    let mut parts = Vec::new();
    if modifiers.platform {
        // The platform key is Command on macOS and the logo key elsewhere,
        // where `Cmd` would instead mean Control.
        parts.push(if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        });
    }
    if modifiers.control {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    parts.push(keystroke.key.as_str());
    parts.join("+").parse().ok()
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    use super::*;

    fn combo(text: &str) -> Option<String> {
        let keystroke = Keystroke::parse(text).expect("valid keystroke");
        recorded_combo(&keystroke).map(|combo| combo.rendered_label())
    }

    #[test]
    fn records_modifier_chords_in_canonical_form() {
        let primary = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        };
        assert_eq!(combo("cmd-c"), Some(format!("{primary}+C")));
        assert_eq!(combo("cmd-shift-z"), Some(format!("{primary}+Shift+Z")));
        assert_eq!(combo("ctrl-alt-left"), Some("Ctrl+Alt+Left".into()));
        assert_eq!(combo("shift-f5"), Some("Shift+F5".into()));
        assert_eq!(combo("ctrl-pageup"), Some("Ctrl+PageUp".into()));
    }

    #[test]
    fn records_bare_keys() {
        assert_eq!(combo("f13"), Some("F13".into()));
        assert_eq!(combo("enter"), Some("Enter".into()));
        assert_eq!(combo("space"), Some("Space".into()));
    }

    #[test]
    fn ignores_keys_outside_the_shortcut_vocabulary() {
        assert_eq!(combo("shift"), None);
        assert_eq!(combo("cmd-f24"), None);
    }

    struct Harness {
        input: Entity<InputState>,
        recorder: Entity<ShortcutRecorder>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child(self.recorder.clone())
        }
    }

    fn press(cx: &mut gpui::VisualTestContext, key: &str) {
        cx.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).expect("valid keystroke"),
            is_held: false,
            prefer_character_input: false,
        });
    }

    #[gpui::test]
    fn records_a_chord_the_app_also_binds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            // The app binds Copy globally (`app/menu.rs`); the recorder must
            // still see the chord instead of the binding swallowing it.
            cx.bind_keys([gpui::KeyBinding::new(
                "cmd-c",
                gpui_component::input::Copy,
                None,
            )]);
        });
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let recorder = cx.new(|cx| ShortcutRecorder::new("test", input.clone(), cx));
            Harness { input, recorder }
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let focus = harness.read(cx).recorder.read(cx).focus.clone();
            window.focus(&focus, cx);
        });

        // A lone modifier keeps listening.
        press(cx, "shift");
        cx.update(|window, cx| {
            let focus = harness.read(cx).recorder.read(cx).focus.clone();
            assert!(focus.is_focused(window));
        });

        press(cx, "cmd-c");
        cx.update(|window, cx| {
            let harness = harness.read(cx);
            let primary = if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Super"
            };
            assert_eq!(harness.input.read(cx).value(), format!("{primary}+C"));
            assert!(!harness.recorder.read(cx).focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn escape_cancels_without_touching_the_field(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let recorder = cx.new(|cx| ShortcutRecorder::new("test", input.clone(), cx));
            Harness { input, recorder }
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let focus = harness.read(cx).recorder.read(cx).focus.clone();
            window.focus(&focus, cx);
        });

        press(cx, "escape");
        cx.update(|window, cx| {
            let harness = harness.read(cx);
            assert_eq!(harness.input.read(cx).value(), "");
            assert!(!harness.recorder.read(cx).focus.is_focused(window));
        });
    }
}
