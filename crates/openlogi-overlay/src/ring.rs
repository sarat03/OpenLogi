//! The ring itself: the GPUI view, and the window it is drawn in.
//!
//! Placement is the interesting part — the panel is centred on the cursor and
//! clamped to the display it came up on, so a ring raised near a screen edge
//! stays whole instead of being cut off.

#[cfg(any(not(target_os = "windows"), test))]
use gpui::{Bounds, Pixels, Point, Size, point};
use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Window, WindowBackgroundAppearance,
    WindowKind, WindowOptions, div, prelude::FluentBuilder as _, px, svg,
};
use openlogi_core::binding::geometry::{self, VisibleItem};
use openlogi_core::binding::{Action, ActionRingSlot, ActionRingTarget};
use openlogi_ipc::{ActionRingInvocation, ActionRingPresentation};
use openlogi_ui::action_icons::RING_CANCEL_ICON;
use openlogi_ui::color;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::ipc::OverlayCommand;
use crate::session::{ClickAwaySession, ShowingRing};

pub(crate) const WINDOW_SIZE: f32 = 360.0;
/// Options+'s geometry is drawn at this fraction, so a ring with a fully
/// expanded folder (two levels out) fits the overlay window.
const SCALE: f32 = WINDOW_SIZE / geometry::CANVAS;
const CENTER: f32 = WINDOW_SIZE / 2.0;

/// The ring's own neutral scale. It floats over whatever is on the desktop, so
/// unlike the settings app it cannot take its surfaces from the OS appearance —
/// it commits to a dark panel and rides its own contrast. Only the accent is
/// shared (`openlogi_ui::color`); these greys are local by nature.
const PANEL: Hsla = neutral(0.06, 0.82);
const SLOT_RESTING: Hsla = neutral(0.16, 0.98);
const FOLDER_OPEN: Hsla = neutral(0.24, 0.98);
const CANCEL_RESTING: Hsla = neutral(0.20, 0.98);
const GLYPH: Hsla = neutral(0.98, 1.0);
const LABEL: Hsla = neutral(0.94, 1.0);
const CANCEL_GLYPH: Hsla = neutral(0.82, 1.0);

const fn neutral(lightness: f32, alpha: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: lightness,
        a: alpha,
    }
}

/// The accent deepened for the dark panel: the brand lightness sits too close to
/// the white glyph a selected slot carries, so the fill drops to `0.48` and the
/// ring around it rises to `0.78`. Both keep the brand hue and saturation.
const SELECTED_FILL_L: f32 = 0.48;
const SELECTED_BORDER_L: f32 = 0.78;

/// What the pointer currently selects, Options+-style: the closest visible
/// item (anywhere in the window), nothing in the centre's dismiss region. A
/// hovered folder opens; leaving it for anything but its own items closes it.
pub(crate) struct RingView {
    invocation: ActionRingInvocation,
    commands: mpsc::UnboundedSender<OverlayCommand>,
    hovered: Option<ActionRingTarget>,
    expanded: Option<ActionRingSlot>,
    /// Publishes click-away identity for exactly this view's lifetime.
    _showing: ShowingRing,
}

impl RingView {
    /// Open a view on `invocation`, reporting interactions through `commands`.
    pub(crate) fn new(
        invocation: ActionRingInvocation,
        commands: mpsc::UnboundedSender<OverlayCommand>,
        live: &Arc<ClickAwaySession>,
    ) -> Self {
        let showing = live.showing(invocation.session_id);
        Self {
            invocation,
            commands,
            hovered: None,
            expanded: None,
            _showing: showing,
        }
    }

    /// The ring session this view is showing.
    pub(crate) const fn session_id(&self) -> u64 {
        self.invocation.session_id
    }

    /// Report this ring cancelled. The window is closed by the caller, which
    /// is the only one holding the handle.
    pub(crate) fn cancel(&self) {
        let _ = self.commands.send(OverlayCommand::Cancel {
            session_id: self.invocation.session_id,
        });
    }

    fn roots(&self) -> Vec<ActionRingSlot> {
        populated(&self.invocation.slots)
    }

    fn is_folder(&self, slot: ActionRingSlot) -> bool {
        self.invocation
            .slots
            .get(&slot)
            .is_some_and(|presentation| !presentation.children.is_empty())
    }

    fn visible(&self) -> Vec<VisibleItem> {
        let roots = self.roots();
        let children = self
            .expanded
            .and_then(|folder| self.invocation.slots.get(&folder))
            .map(|folder| populated(&folder.children))
            .unwrap_or_default();
        geometry::visible_items(
            &roots,
            self.expanded.map(|folder| (folder, children.as_slice())),
        )
    }

    fn presentation(&self, target: ActionRingTarget) -> Option<&ActionRingPresentation> {
        let root = self.invocation.slots.get(&target.slot)?;
        match target.child {
            None => Some(root),
            Some(child) => root.children.get(&child),
        }
    }

    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let cursor = (
            (f32::from(event.position.x) - CENTER) / SCALE,
            (f32::from(event.position.y) - CENTER) / SCALE,
        );
        let hit = geometry::hit(cursor, &self.visible());
        if hit == self.hovered {
            return;
        }
        self.hovered = hit;
        if let Some(target) = hit {
            match target.child {
                None if self.is_folder(target.slot) => self.expanded = Some(target.slot),
                None => self.expanded = None,
                Some(_) => {}
            }
            let _ = self.commands.send(OverlayCommand::Hover {
                session_id: self.invocation.session_id,
                target,
            });
        }
        cx.notify();
    }

    /// A click anywhere selects what the pointer selects: an action executes,
    /// a folder stays open, and the centre (or nothing) dismisses the ring.
    fn clicked(&self, window: &mut Window) {
        let session_id = self.invocation.session_id;
        match self.hovered {
            Some(target) if target.child.is_none() && self.is_folder(target.slot) => {}
            Some(target) => {
                let _ = self
                    .commands
                    .send(OverlayCommand::Activate { session_id, target });
                window.remove_window();
            }
            None => {
                let _ = self.commands.send(OverlayCommand::Cancel { session_id });
                window.remove_window();
            }
        }
    }

    fn item_element(&self, item: VisibleItem) -> Option<gpui::AnyElement> {
        let presentation = self.presentation(item.target)?;
        let selected = self.hovered == Some(item.target);
        let open_folder = item.target.child.is_none() && self.expanded == Some(item.target.slot);
        let size = 2.0 * geometry::ITEM_RADIUS * SCALE;
        Some(
            div()
                .absolute()
                .left(px(CENTER + item.at.0 * SCALE - size / 2.0))
                .top(px(CENTER + item.at.1 * SCALE - size / 2.0))
                .size(px(size))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(if selected {
                    color::accent_at_lightness(SELECTED_FILL_L)
                } else if open_folder {
                    FOLDER_OPEN
                } else {
                    SLOT_RESTING
                })
                .when(selected, |slot| {
                    slot.border_2()
                        .border_color(color::accent_at_lightness(SELECTED_BORDER_L))
                })
                .shadow_md()
                .text_color(GLYPH)
                .child(
                    svg()
                        .path(presentation.icon.asset_path())
                        .size(px(size * 0.45))
                        .text_color(GLYPH),
                )
                .into_any_element(),
        )
    }
}

/// The populated positions of `slots`, in display order.
fn populated(
    slots: &std::collections::BTreeMap<ActionRingSlot, ActionRingPresentation>,
) -> Vec<ActionRingSlot> {
    ActionRingSlot::ALL
        .into_iter()
        .filter(|slot| slots.contains_key(slot))
        .collect()
}

fn label_text(presentation: &ActionRingPresentation) -> SharedString {
    // User-authored labels render verbatim: passing them through the
    // localization table would translate any label that happens to collide
    // with a known key ("Copy" → "Copier" under fr).
    let label = if presentation.literal {
        presentation.label.clone()
    } else if let Some(key) = Action::translation_key_for_label(&presentation.label) {
        rust_i18n::t!(key).into_owned()
    } else {
        presentation.label.clone()
    };
    SharedString::from(label)
}

impl Render for RingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hovered_label = self
            .hovered
            .and_then(|target| self.presentation(target))
            .map(label_text);
        let items = self
            .visible()
            .into_iter()
            .filter_map(|item| self.item_element(item))
            .collect::<Vec<_>>();
        let panel = 2.0 * (geometry::LEVEL_SPACING + 1.5 * geometry::ITEM_RADIUS) * SCALE;
        let cancel = 2.0 * geometry::CENTER_RADIUS * SCALE * 1.5;

        div()
            .id("ring-root")
            .relative()
            .size_full()
            .on_mouse_move(cx.listener(|this, event, _, cx| this.pointer_moved(event, cx)))
            .child(
                div()
                    .absolute()
                    .left(px(CENTER - panel / 2.0))
                    .top(px(CENTER - panel / 2.0))
                    .size(px(panel))
                    .rounded_full()
                    .bg(PANEL)
                    .shadow_lg(),
            )
            .children(items)
            .child(
                div()
                    .absolute()
                    .left(px(CENTER - cancel / 2.0))
                    .top(px(CENTER - cancel / 2.0))
                    .size(px(cancel))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(CANCEL_RESTING)
                    .text_color(CANCEL_GLYPH)
                    .child(
                        svg()
                            .path(RING_CANCEL_ICON)
                            .size(px(cancel * 0.45))
                            .flex_none(),
                    ),
            )
            .when_some(hovered_label, |ring, label| {
                ring.child(
                    div()
                        .absolute()
                        .left(px(CENTER - 80.0))
                        .top(px(CENTER + cancel / 2.0 + 6.0))
                        .w(px(160.0))
                        .text_center()
                        .text_sm()
                        .text_color(LABEL)
                        .child(label),
                )
            })
            .on_click(cx.listener(|this, _, window, _| this.clicked(window)))
    }
}

/// Appearance shared by the platform-specific placement paths.
pub(crate) fn ring_window_options() -> WindowOptions {
    WindowOptions {
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("openlogi-action-ring".to_string()),
        ..WindowOptions::default()
    }
}

#[cfg(any(not(target_os = "windows"), test))]
pub(crate) fn clamp_window_origin(
    desired: Point<Pixels>,
    window_size: Size<Pixels>,
    display: Bounds<Pixels>,
) -> Point<Pixels> {
    let max = point(
        display.right() - window_size.width,
        display.bottom() - window_size.height,
    );
    desired.clamp(&display.origin, &max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_origin_is_clamped_to_the_display() {
        let display = Bounds::new(point(px(100.0), px(50.0)), Size::new(px(800.0), px(600.0)));
        let size = Size::new(px(400.0), px(400.0));
        assert_eq!(
            clamp_window_origin(point(px(-50.0), px(-50.0)), size, display),
            point(px(100.0), px(50.0))
        );
        assert_eq!(
            clamp_window_origin(point(px(700.0), px(500.0)), size, display),
            point(px(500.0), px(250.0))
        );
    }

    #[test]
    fn overlay_origin_stays_cursor_centered_away_from_edges() {
        let display = Bounds::new(Point::default(), Size::new(px(1600.0), px(1000.0)));
        let desired = point(px(600.0), px(300.0));
        assert_eq!(
            clamp_window_origin(desired, Size::new(px(400.0), px(400.0)), display),
            desired
        );
    }
}
