//! Actions Ring editor for the active device: eight root positions, each an
//! action or a folder of eight more.

mod action_icons;
mod editor;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Icon, IconName, Selectable as _, button::Button, h_flex, input::InputState, tooltip::Tooltip,
    v_flex,
};
use openlogi_core::binding::{
    ActionRingConfig, ActionRingEntry, ActionRingIcon, ActionRingLayout, ActionRingSlot,
    ActionRingTarget,
};
use openlogi_ui::action_icons::RING_CANCEL_ICON;

use self::action_icons::action_icon_path;
use self::editor::action_library;
use crate::state::{AppState, StateEvent, StateEvents};
use crate::ui::action::localized_action_label;
use crate::ui::theme::{self, Palette, Typography as _};

/// Stateful Actions Ring editor. Ring configuration itself lives in
/// [`AppState`]; this entity owns selection and editor input state.
pub struct ActionRingPanel {
    focus_handle: FocusHandle,
    selected_slot: ActionRingSlot,
    /// The item being edited inside the selected slot, when that slot is a
    /// folder.
    selected_child: ActionRingSlot,
    application_input: Option<Entity<InputState>>,
    shortcut_input: Option<Entity<InputState>>,
    library_scroll: ScrollHandle,
    #[expect(dead_code, reason = "held to keep the AppState subscription alive")]
    state_obs: Subscription,
}

impl ActionRingPanel {
    /// Create the editor and repaint it after any config/device change.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state_obs =
            AppState::repaint_on(cx, |event| matches!(event, StateEvent::BindingsChanged(_)));
        Self {
            focus_handle: cx.focus_handle(),
            selected_slot: ActionRingSlot::Top,
            selected_child: ActionRingSlot::Top,
            application_input: None,
            shortcut_input: None,
            library_scroll: ScrollHandle::new(),
            state_obs,
        }
    }
}

impl ActionRingPanel {
    /// The position the action library edits: the selected root slot, or the
    /// selected item inside it when it is a folder.
    fn editing_target(&self, layout: &ActionRingLayout) -> ActionRingTarget {
        if layout.folders().contains_key(&self.selected_slot) {
            ActionRingTarget::child(self.selected_slot, self.selected_child)
        } else {
            ActionRingTarget::root(self.selected_slot)
        }
    }

    fn ring_column(
        &self,
        layout: &ActionRingLayout,
        view: &Entity<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        v_flex()
            .gap_3()
            .items_center()
            .child(ring_preview(layout, self.selected_slot, view, pal))
            .child(folder_controls(
                layout,
                self.selected_slot,
                self.selected_child,
                view,
                pal,
            ))
    }
}

impl Focusable for ActionRingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ActionRingPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        let (ring, layout) = action_ring_editor_state(cx);
        let haptics_supported = current_device_supports_haptics(cx);
        let application_input = editor_input(
            &mut self.application_input,
            tr!("action_ring.application_folder_path_or_url"),
            window,
            cx,
        );
        let shortcut_input = editor_input(
            &mut self.shortcut_input,
            tr!("action_ring.shortcut_e_g_cmd_plus_shift_plus_p"),
            window,
            cx,
        );
        let view = cx.entity();
        let target = self.editing_target(&layout);

        v_flex()
            .w_full()
            .gap_4()
            .tab_group()
            .track_focus(&self.focus_handle)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_subheading()
                            .child(tr!("action_ring.actions_ring")),
                    )
                    .child(
                        div()
                            .text_caption()
                            .text_color(pal.text_muted)
                            .child(tr!("action_ring.action_ring_description")),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_center()
                    .gap_4()
                    .child(self.ring_column(&layout, &view, pal))
                    .child(action_library(
                        target,
                        layout.entry_at(target),
                        &application_input,
                        &shortcut_input,
                        &self.library_scroll,
                        pal,
                    )),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        v_flex()
                            .child(div().text_body().child(tr!("action_ring.actions_ring")))
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(tr!("action_ring.open_at_the_current_cursor_position")),
                            ),
                    )
                    .child(toggle_button(
                        "ring-enabled",
                        ring.enabled,
                        AppState::commit_action_ring_enabled,
                    )),
            )
            .when(haptics_supported, |panel| {
                panel.child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            v_flex()
                                .child(div().text_body().child(tr!("action_ring.haptic_feedback")))
                                .child(
                                    div()
                                        .text_caption()
                                        .text_color(pal.text_muted)
                                        .child(tr!("action_ring.action_ring_haptic_description")),
                                ),
                        )
                        .child(toggle_button(
                            "ring-haptics",
                            ring.haptics,
                            AppState::commit_action_ring_haptics,
                        )),
                )
            })
    }
}

fn action_ring_editor_state(cx: &Context<ActionRingPanel>) -> (ActionRingConfig, ActionRingLayout) {
    AppState::try_read(cx).map_or_else(
        || {
            let ring = ActionRingConfig::default();
            let layout = ring.default.clone();
            (ring, layout)
        },
        |state| {
            let ring = state.current_action_ring();
            let layout = state.current_action_ring_layout();
            (ring, layout)
        },
    )
}

fn editor_input(
    state: &mut Option<Entity<InputState>>,
    placeholder: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<ActionRingPanel>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let state = state
        .get_or_insert_with(|| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()))
        })
        .clone();
    // Callers pass a per-render `tr!` string, so a cached input follows a live
    // language switch instead of keeping the placeholder it was built with.
    crate::ui::components::localize_placeholder(&state, placeholder, window, cx);
    state
}

fn current_device_supports_haptics(cx: &Context<ActionRingPanel>) -> bool {
    AppState::try_read(cx).is_some_and(|state| {
        state.current_record().is_some_and(|record| {
            record
                .capabilities
                .unwrap_or_else(|| {
                    openlogi_core::device::Capabilities::presumed_from_kind(record.kind)
                })
                .haptic_feedback
        })
    })
}

fn toggle_button(
    id: &'static str,
    enabled: bool,
    commit: impl Fn(&mut AppState, bool) -> StateEvents + 'static,
) -> Button {
    Button::new(id)
        .compact()
        .label(if enabled {
            tr!("common.on")
        } else {
            tr!("common.off")
        })
        .selected(enabled)
        .on_click(move |_, _, cx| AppState::apply(cx, |state| commit(state, !enabled)))
}

const PREVIEW_SIZE: f32 = 320.0;
const PREVIEW_RADIUS: f32 = 106.0;
const PREVIEW_SLOT_SIZE: f32 = 50.0;

fn ring_preview(
    layout: &ActionRingLayout,
    selected_slot: ActionRingSlot,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .size(px(PREVIEW_SIZE))
        .child(
            div()
                .absolute()
                .left(px(24.0))
                .top(px(24.0))
                .size(px(PREVIEW_SIZE - 48.0))
                .rounded_full()
                .border_1()
                .border_color(pal.border)
                .bg(pal.panel),
        )
        .child(
            div()
                .absolute()
                .left(px(PREVIEW_SIZE / 2.0 - 24.0))
                .top(px(PREVIEW_SIZE / 2.0 - 24.0))
                .size(px(48.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(pal.muted)
                .text_color(pal.text_muted)
                .child(svg().path(RING_CANCEL_ICON).size(px(20.0)).flex_none()),
        )
        .children(ActionRingSlot::ALL.into_iter().map(|slot| {
            let content = match layout.folders().get(&slot) {
                Some(folder) => SlotContent::Folder(folder.icon),
                None => layout
                    .slots
                    .get(&slot)
                    .map_or(SlotContent::Empty, SlotContent::Action),
            };
            let (left, top) = slot.placement(PREVIEW_SIZE, PREVIEW_RADIUS, PREVIEW_SLOT_SIZE);
            let select = view.clone();
            slot_button(
                ("action-ring-slot", slot.index()),
                content,
                selected_slot == slot,
                PREVIEW_SLOT_SIZE,
                pal,
            )
            .absolute()
            .left(px(left))
            .top(px(top))
            .on_click(move |_, _, cx| {
                select.update(cx, |panel, cx| {
                    panel.selected_slot = slot;
                    panel.selected_child = ActionRingSlot::Top;
                    cx.notify();
                });
            })
        }))
}

/// The folder toggle for the selected root position and, when it is a
/// folder, the folder's own eight positions to pick the item being edited.
fn folder_controls(
    layout: &ActionRingLayout,
    selected_slot: ActionRingSlot,
    selected_child: ActionRingSlot,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement {
    let folder = layout.folders().get(&selected_slot);
    let is_folder = folder.is_some();
    v_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new("ring-folder-toggle")
                .compact()
                .label(if is_folder {
                    tr!("action_ring.make_action")
                } else {
                    tr!("action_ring.make_folder")
                })
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| {
                        state.commit_action_ring_folder(selected_slot, !is_folder)
                    });
                }),
        )
        .when_some(folder, |controls, folder| {
            controls
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(tr!("action_ring.folder_items")),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .children(ActionRingSlot::ALL.into_iter().map(|child| {
                            let select = view.clone();
                            slot_button(
                                ("action-ring-folder-item", child.index()),
                                folder
                                    .items
                                    .get(&child)
                                    .map_or(SlotContent::Empty, SlotContent::Action),
                                selected_child == child,
                                FOLDER_ITEM_SIZE,
                                pal,
                            )
                            .on_click(move |_, _, cx| {
                                select.update(cx, |panel, cx| {
                                    panel.selected_child = child;
                                    cx.notify();
                                });
                            })
                        })),
                )
        })
}

const FOLDER_ITEM_SIZE: f32 = 32.0;

/// What one editor position shows.
#[derive(Clone, Copy)]
enum SlotContent<'a> {
    Empty,
    Action(&'a ActionRingEntry),
    Folder(Option<ActionRingIcon>),
}

fn slot_button(
    id: impl Into<gpui::ElementId>,
    content: SlotContent<'_>,
    selected: bool,
    size: f32,
    pal: Palette,
) -> BaseButton {
    let (label, icon_path) = match content {
        SlotContent::Empty => (tr!("action_ring.empty_slot").to_string(), None),
        SlotContent::Action(entry) => (
            localized_action_label(entry.action()).to_string(),
            Some(entry.custom_icon().map_or_else(
                || action_icon_path(entry.action()),
                ActionRingIcon::asset_path,
            )),
        ),
        SlotContent::Folder(icon) => (
            tr!("action_ring.ring_folder").to_string(),
            Some(icon.unwrap_or(ActionRingIcon::Layers).asset_path()),
        ),
    };
    let is_folder = matches!(content, SlotContent::Folder(_));
    let accessible_label = label.clone();

    BaseButton::new(id)
        .selected(selected)
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2()
        .when(is_folder, Styled::border_dashed)
        .border_color(if selected {
            rgb(theme::ACCENT_BLUE).into()
        } else {
            pal.border
        })
        .bg(if selected {
            theme::accent_tint()
        } else {
            pal.control
        })
        .text_color(if selected {
            pal.text_primary
        } else {
            pal.text_muted
        })
        .cursor_pointer()
        .accessibility_label(accessible_label)
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .when_some(icon_path, |button, path| {
            button.child(
                svg()
                    .path(path)
                    .size(px(size * 0.4))
                    .text_color(if selected {
                        pal.text_primary
                    } else {
                        pal.text_muted
                    }),
            )
        })
        .when(icon_path.is_none(), |button| {
            button.child(Icon::new(IconName::Plus).size_4())
        })
        .hover(move |button| {
            button.bg(if selected {
                theme::accent_tint_hover()
            } else {
                pal.control_hover
            })
        })
        .focus_visible(move |button| {
            button
                .border_color(rgb(theme::ACCENT_BLUE))
                .bg(if selected {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
        })
}
